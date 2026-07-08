//! Page `/loi/{code}/{num}[/{date}]` (LawArticlePage, SEO-critique). Calquée sur
//! [`crate::pages::decision_page`] :
//!
//! - Article (en-tête, méta, corps, timeline) : `Resource::new_blocking` ⇒ le SSR
//!   attend la résolution avant d'émettre le HTML (`<Title>`/`<Meta>`/JSON-LD
//!   crawlables). Le `LawArticleResponse` porte sa propre timeline (`versions`).
//! - Décisions citantes : `Resource` non bloquante + `<Suspense>` ⇒ streamées
//!   après le shell.
//! - `Cache-Control` (SSR) posé selon le statut (200 → 7 j CDN, 404/400/422 →
//!   5 min, 5xx → no-store), comme la page décision.

pub mod data;

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Script, Title};
use leptos_router::components::A;
use lj_dtos::{ArticleNeighbor, CitingDecisionHit, LawArticleResponse, LawArticleVersion};

use crate::components::decision::DecisionLayout;
use crate::helpers::{format_article_num, format_iso_date, format_juridiction};
use crate::seo::law::{article_canonical_url, article_meta_description, build_article_json_ld};
use crate::seo::OG_IMAGE;

use data::{article_key, fetch_article, fetch_citing, sendable, CitingResult, PageError};

#[component]
pub fn LawArticlePage() -> impl IntoView {
    let key = article_key();

    // Article bloquant (SEO dans le document initial).
    let article = Resource::new_blocking(move || key.get(), |key| sendable(fetch_article(key)));
    // Décisions citantes non bloquantes (streamées via <Suspense>).
    let citing = Resource::new(
        move || {
            let k = key.get();
            (k.code, k.num)
        },
        |(code, num)| sendable(fetch_citing(code, num)),
    );

    view! {
        <Suspense fallback=LawSkeleton>
            {move || Suspend::new(async move {
                match article.await {
                    Ok(article) => {
                        set_cache_control(200);
                        Either::Left(view! { <LawArticleLoaded article=article citing=citing /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <LawError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

/// Pose le `Cache-Control` de la réponse document SSR selon le statut. No-op
/// côté hydrate. Aligné sur `decision_page::set_cache_control` (200 → 7 j ;
/// erreurs client → 5 min ; 5xx → no-store).
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

/// Page d'erreur (référence invalide / 404 / autre). Pose `robots noindex`.
#[component]
fn LawError(err: PageError) -> impl IntoView {
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    let title = "Article introuvable — LibreJustice";
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

/// Squelette de chargement (en-tête + corps + colonnes).
#[component]
fn LawSkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <Skeleton class="h-4 w-1/3" />
            <Skeleton class="h-9 w-2/3" />
            <Skeleton class="h-4 w-1/2" />
            <div class="mt-4 flex flex-col gap-3">
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-4 w-11/12" />
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-4 w-3/5" />
            </div>
        </div>
    }
}

/// Titre humain d'un article (« Code civil, article 1240 »). Le `code` d'URL
/// étant un slug, on préfère le `titre_text` (fil d'Ariane) quand il existe.
/// Partagé avec la hover card d'article (ADR 0168).
pub(crate) fn article_title(article: &LawArticleResponse) -> String {
    format!(
        "Article {} du {}",
        format_article_num(&article.num),
        code_display_name(article)
    )
}

/// Nom affichable du code : le titre humain (`code_title`, ex. « Code de la famille
/// sénégalais ») quand l'API le fournit ; à défaut, un libellé dérivé du slug `code`
/// (tirets → espaces), forcément approximatif.
fn code_display_name(article: &LawArticleResponse) -> String {
    article
        .code_title
        .clone()
        .unwrap_or_else(|| article.code.replace('-', " "))
}

/// Page chargée : SEO (title/meta/OG/canonical/JSON-LD) + en-tête, méta, corps,
/// timeline, décisions citantes.
#[component]
fn LawArticleLoaded(article: LawArticleResponse, citing: Resource<CitingResult>) -> impl IntoView {
    let title = article_title(&article);
    let description = article_meta_description(&article, &title);
    // URL canonique sur la clé `numKey` (forme résolue en lookup exact, ADR 0123 §2).
    let url = article_canonical_url(&article.code, &article.num_key);
    let jsonld = serde_json::to_string(&build_article_json_ld(&article, &title, &description))
        .unwrap_or_else(|_| "{}".to_string());
    let page_title = format!("{title} — LibreJustice");

    let in_force_since = format_iso_date(Some(&article.date_debut));
    let code_href = format!("/loi/{}", article.code);
    let code_name = code_display_name(&article);

    let nav_view = view! { <ArticleSideNav article=article.clone() /> }.into_any();
    let main_view = view! {
        <article class="flex flex-col gap-10">
            <header class="flex flex-col gap-2">
                <A
                    href=code_href
                    attr:class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)] underline-offset-4 hover:text-[var(--color-accent)]"
                >
                    {code_name}
                </A>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{title.clone()}</h1>
                <p class="text-sm text-[var(--color-ink-muted)]">
                    {format!("En vigueur depuis le {in_force_since}")}
                </p>
            </header>
            <ArticleBody article=article.clone() />
            <ArticleMeta article=article.clone() />
        </article>
    }
    .into_any();
    let citing_view = view! { <CitingSection citing=citing /> }.into_any();

    view! {
        <Title text=page_title />
        <Meta name="description" content=description.clone() />
        <Meta property="og:type" content="article" />
        <Meta property="og:site_name" content="LibreJustice" />
        <Meta property="og:title" content=title.clone() />
        <Meta property="og:description" content=description.clone() />
        <Meta property="og:url" content=url.clone() />
        <Meta property="og:locale" content="fr_FR" />
        <Meta property="og:image" content=OG_IMAGE />
        <Meta name="twitter:card" content="summary_large_image" />
        <Link rel="canonical" href=url />
        <Script type_="application/ld+json">{jsonld}</Script>
        <DecisionLayout toc=nav_view main=main_view similar=citing_view />
    }
}

/// Colonne gauche : l'article dans son contexte — voisins du chapitre puis
/// versions, chacun en points sur rail fin (même idiome que le sommaire et la
/// chronologie de la page décision ; le point plein accent marque la page /
/// version courante).
#[component]
fn ArticleSideNav(article: LawArticleResponse) -> impl IntoView {
    let article_versions = article.clone();
    view! {
        <nav
            aria-label="Contexte de l'article"
            class="flex flex-col gap-8 lg:sticky lg:top-20 lg:self-start"
        >
            <ArticleContext article=article />
            <Timeline article=article_versions />
        </nav>
    }
}

/// Bloc à points sur rail (idiome sommaire/chronologie) : en-tête petites
/// capitales + liste, chaque entrée ancrée au rail par un point (plein accent
/// = entrée courante).
fn rail_block(title: &'static str, items: Vec<AnyView>) -> AnyView {
    view! {
        <div class="flex flex-col gap-3">
            <p class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            <div class="relative">
                <div
                    aria-hidden="true"
                    class="absolute bottom-2 left-[7px] top-2 w-[2px] rounded-full bg-[var(--color-rule)]"
                />
                <ol class="flex flex-col gap-3">{items}</ol>
            </div>
        </div>
    }
    .into_any()
}

/// Entrée d'un bloc à rail : point + contenu.
fn rail_item(current: bool, body: AnyView) -> AnyView {
    let dot_class = if current {
        "absolute left-[3px] top-[5px] h-[10px] w-[10px] rounded-full border-2 border-[var(--color-accent)] bg-[var(--color-accent)]"
    } else {
        "absolute left-[3px] top-[5px] h-[10px] w-[10px] rounded-full border-2 border-[var(--color-rule)] bg-[var(--color-parchment)]"
    };
    view! {
        <li class="relative pl-7">
            <span aria-hidden="true" class=dot_class />
            {body}
        </li>
    }
    .into_any()
}

/// (href, libellé) de la section descriptive de la source sur `/sources`
/// (ADR 0114). Ancre par provenance ; fallback page `/sources` sans ancre.
fn source_descriptor(source: &str) -> (String, &'static str) {
    let (anchor, label) = match source {
        "legifrance" => ("dila", "Base LEGI · DILA"),
        "jorf" => ("dila", "Journal officiel · DILA"),
        _ => ("", "Données & sources"),
    };
    let href = if anchor.is_empty() {
        "/sources".to_string()
    } else {
        format!("/sources#{anchor}")
    };
    (href, label)
}

/// Bloc méta : état, dates de validité, identifiant LEGIARTI, lien Légifrance
/// versionné.
#[component]
fn ArticleMeta(article: LawArticleResponse) -> impl IntoView {
    let date_fin = article
        .date_fin
        .as_deref()
        .map(|d| format_iso_date(Some(d)))
        .unwrap_or_else(|| "en vigueur".to_string());
    let (source_href, source_label) = source_descriptor(&article.source);
    // Fraîcheur / autorité du diffuseur (ADR 0129). `source_asof` jamais inconnue en
    // pratique (au pire date de get). `source_authority` = axe distinct de `translation`.
    let freshness = article
        .source_asof
        .as_deref()
        .map(|d| format_iso_date(Some(d)));
    let source_authority = article.source_authority.clone();
    let source_url = article.source_url.clone().filter(|u| !u.is_empty());
    let source_upstream = article
        .source_upstream_url
        .clone()
        .filter(|u| !u.is_empty());

    view! {
        <section
            aria-label="Métadonnées"
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6"
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Métadonnées"</h2>
            <dl class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
                <MetaField label="État" value=article.etat mono=false />
                <MetaField
                    label="Début de validité"
                    value=format_iso_date(Some(&article.date_debut))
                    mono=false
                />
                <MetaField label="Fin de validité" value=date_fin mono=false />
                <MetaField label="Identifiant" value=article.legiarti mono=true />
                {freshness
                    .map(|f| view! { <MetaField label="Fraîcheur de la source" value=f mono=false /> })}
                <MetaField label="Diffuseur" value=source_authority mono=false />
            </dl>
            <div class="mt-5 flex flex-wrap items-center gap-x-6 gap-y-2 border-t border-[var(--color-rule)] pt-4">
                {source_url
                    .map(|u| {
                        view! {
                            <a
                                href=u
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                            >
                                "Voir la source"
                                <span aria-hidden="true">"↗"</span>
                            </a>
                        }
                    })}
                {source_upstream
                    .map(|u| {
                        view! {
                            <a
                                href=u
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink-subtle)] underline-offset-4 hover:text-[var(--color-accent)]"
                            >
                                "Source d'origine"
                                <span aria-hidden="true">"↗"</span>
                            </a>
                        }
                    })}
                <A
                    href=source_href
                    attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink-muted)] underline-offset-4 hover:text-[var(--color-accent)]"
                >
                    {format!("Source : {source_label}")}
                </A>
            </div>
        </section>
    }
}

/// Lecture en contexte (ADR 0114) : voisins de l'article dans sa division (ou
/// fenêtre), l'article courant au point plein. Masqué si pas de contexte (≤ 1
/// entrée = seulement l'article lui-même).
#[component]
fn ArticleContext(article: LawArticleResponse) -> impl IntoView {
    let context = article.context;
    if context.len() <= 1 {
        return ().into_any();
    }
    let code = article.code.clone();
    let items = context
        .into_iter()
        .map(|n| context_item(n, &code))
        .collect();
    rail_block("Dans le même chapitre", items)
}

fn context_item(neighbor: ArticleNeighbor, code: &str) -> AnyView {
    let label = format!("Article {}", format_article_num(&neighbor.num));
    let abrogated = neighbor.etat != "VIGUEUR";
    if neighbor.current {
        let body = view! {
            <span aria-current="page" class="block text-sm font-medium leading-snug text-[var(--color-ink)]">
                {label}
            </span>
        }
        .into_any();
        return rail_item(true, body);
    }
    // Lien sur la clé canonique (`numKey`), résolue en lookup exact (ADR 0123 §2).
    let href = format!("/loi/{code}/{}", neighbor.num_key);
    let cls = if abrogated {
        "block text-sm leading-snug text-[var(--color-ink-subtle)] line-through no-underline hover:text-[var(--color-accent)]"
    } else {
        "block text-sm leading-snug text-[var(--color-accent)] no-underline hover:underline"
    };
    let body = view! { <A href=href attr:class=cls>{label}</A> }.into_any();
    rail_item(false, body)
}

#[component]
fn MetaField(label: &'static str, value: String, mono: bool) -> impl IntoView {
    let dd_class = if mono {
        "font-mono text-sm text-[var(--color-ink)] break-all"
    } else {
        "text-sm text-[var(--color-ink)]"
    };
    view! {
        <div class="flex flex-col gap-1">
            <dt class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {label}
            </dt>
            <dd class=dd_class>{value}</dd>
        </div>
    }
}

/// Corps de l'article : texte principal (+ texte original en accordéon si trad.) et,
/// sous le texte, le `nota` = apparat éditorial (ADR 0135) : « Nota » officielle
/// Légifrance (entrée en vigueur, QPC) ou jurisprudence/doctrine + renvois « voir
/// aussi » des éditions annotées. Rendu dans un encart distinct, non normatif.
#[component]
fn ArticleBody(article: LawArticleResponse) -> impl IntoView {
    let texte = article
        .texte
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Texte indisponible.".to_string());
    // Badge de provenance (ADR 0116) : on signale une traduction non officielle ou
    // automatique ; le texte officiel (cas par défaut) ne porte pas de badge.
    let badge = match article.translation.as_str() {
        "non_officiel" => Some(("Traduction non officielle", false)),
        "automatique" => Some(("Traduction automatique — non vérifiée", true)),
        _ => None,
    }
    .map(|(label, warn)| {
        let tone = if warn {
            "border-[var(--color-accent)] text-[var(--color-accent)]"
        } else {
            "border-[var(--color-rule)] text-[var(--color-ink-subtle)]"
        };
        view! {
            <span class=format!(
                "inline-flex w-fit items-center rounded-full border px-2.5 py-0.5 text-xs {tone}",
            )>{label}</span>
        }
    });
    // Texte original (ADR 0116) : couche vérification/vérité, en accordéon.
    let lang_orig = article.lang_original.clone();
    let original = article
        .texte_original
        .filter(|t| !t.trim().is_empty())
        .map(|orig| {
            let dir = if lang_orig.as_deref() == Some("ar") { "rtl" } else { "ltr" };
            let label = match lang_orig.as_deref() {
                Some("ar") => "Texte original (arabe)".to_string(),
                Some(l) => format!("Texte original ({l})"),
                None => "Texte original".to_string(),
            };
            view! {
                <details class="mt-6 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-3">
                    <summary class="cursor-pointer text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                        {label}
                    </summary>
                    <div
                        dir=dir
                        class="mt-2 whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]"
                    >
                        {orig}
                    </div>
                </details>
            }
        });
    let fil = article
        .titre_text
        .filter(|t| !t.trim().is_empty())
        .map(|fil| {
            view! {
                <p class="text-xs text-[var(--color-ink-subtle)]">{fil}</p>
            }
        });
    // Apparat éditorial (ADR 0135) : encart distinct sous le texte, ton secondaire pour
    // marquer le caractère non normatif (Nota officielle / jurisprudence / renvois).
    let nota = article
        .nota
        .filter(|t| !t.trim().is_empty())
        .map(|n| {
            view! {
                <aside class="mt-2 rounded-md border-l-2 border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-3">
                    <p class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                        "Notes"
                    </p>
                    <div class="mt-2 whitespace-pre-line text-sm leading-relaxed text-[var(--color-ink-subtle)]">
                        {n}
                    </div>
                </aside>
            }
        });

    view! {
        <article class="flex flex-col gap-4">
            {fil}
            {badge}
            <div class="whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]">
                {texte}
            </div>
            {original}
            {nota}
        </article>
    }
}

/// Versions de l'article, du rail : chaque entrée lie vers `…/{date_debut}`,
/// la version servie au point plein.
#[component]
fn Timeline(article: LawArticleResponse) -> impl IntoView {
    if article.versions.is_empty() {
        return ().into_any();
    }
    let code = article.code.clone();
    // Lien sur la clé canonique (`numKey`), résolue en lookup exact (ADR 0123 §2).
    let num = article.num_key.clone();
    let current = article.legiarti.clone();
    let rows = article
        .versions
        .into_iter()
        .map(|v| timeline_item(v, &code, &num, &current))
        .collect();
    rail_block("Versions", rows)
}

fn timeline_item(version: LawArticleVersion, code: &str, num: &str, current: &str) -> AnyView {
    let is_current = version.legiarti == current;
    let span = match version.date_fin.as_deref() {
        Some(fin) => format!(
            "{} – {}",
            format_iso_date(Some(&version.date_debut)),
            format_iso_date(Some(fin))
        ),
        None => format!("depuis le {}", format_iso_date(Some(&version.date_debut))),
    };
    let etat = view! {
        <span class="block text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
            {version.etat}
        </span>
    };
    if is_current {
        let body = view! {
            <span aria-current="true" class="block">
                <span class="block text-sm font-medium leading-snug text-[var(--color-ink)]">
                    {span}
                </span>
                {etat}
            </span>
        }
        .into_any();
        return rail_item(true, body);
    }
    let href = format!("/loi/{code}/{num}/{}", version.date_debut);
    let body = view! {
        <A href=href attr:class="group block no-underline">
            <span class="block text-sm leading-snug text-[var(--color-accent)] group-hover:underline">
                {span}
            </span>
            {etat}
        </A>
    }
    .into_any();
    rail_item(false, body)
}

/// Décisions citant l'article (streamées), colonne droite en cartes — même
/// gabarit que « Décisions similaires » de la page décision. Erreur ⇒ message
/// discret ; liste vide ⇒ colonne vide.
#[component]
fn CitingSection(citing: Resource<CitingResult>) -> impl IntoView {
    view! {
        <aside
            aria-label="Décisions citantes"
            class="flex flex-col gap-4 lg:sticky lg:top-20 lg:self-start"
        >
            <Suspense fallback=move || {
                view! {
                    <p class="text-sm text-[var(--color-ink-subtle)]">
                        "Chargement des décisions…"
                    </p>
                }
            }>
                {move || Suspend::new(async move {
                    let resolved = citing.await;
                    citing_view(resolved)
                })}
            </Suspense>
        </aside>
    }
}

fn citing_view(resolved: CitingResult) -> AnyView {
    if let Some(err) = resolved.error {
        return view! {
            <p class="text-sm text-[var(--color-ink-subtle)]">
                {format!("Décisions citantes indisponibles ({err}).")}
            </p>
        }
        .into_any();
    }
    if resolved.hits.is_empty() {
        return ().into_any();
    }
    let cards = resolved
        .hits
        .into_iter()
        .map(|hit| view! { <CitingCard hit=hit /> })
        .collect_view();
    view! {
        <h2 class="font-sans text-base text-[var(--color-ink)]">"Décisions citant cet article"</h2>
        <ul class="flex flex-col gap-3">{cards}</ul>
    }
    .into_any()
}

#[component]
fn CitingCard(hit: CitingDecisionHit) -> impl IntoView {
    use crate::components::hover_preview::{HoverPreview, PreviewKind};
    let kind = PreviewKind::Decision { id: hit.id.clone() };
    let href = format!("/decision/{}", hit.id);
    let date = hit
        .date_lecture
        .as_deref()
        .map(|d| format_iso_date(Some(d)));
    let jur = format_juridiction(hit.juridiction_type);
    view! {
        <li class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-3">
            <div class="flex flex-col gap-1">
                <h3 class="font-sans text-sm leading-snug text-[var(--color-ink)]">
                    // Hover card de décision sur le lien (ADR 0168) — le hit
                    // citant est léger (titre + date), la carte apporte
                    // solution/portée/résumé.
                    <HoverPreview kind=kind>
                        <A
                            href=href
                            attr:class="no-underline transition-colors hover:text-[var(--color-accent)]"
                        >
                            {hit.title}
                        </A>
                    </HoverPreview>
                </h3>
                <p class="text-xs text-[var(--color-ink-subtle)]">
                    {match date {
                        Some(date) => format!("{jur} · {date}"),
                        None => jur.to_string(),
                    }}
                </p>
            </div>
        </li>
    }
}
