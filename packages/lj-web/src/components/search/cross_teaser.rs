//! Ponts croisés entre les deux moteurs : bloc « Dans les textes » dans le
//! rail de `/decision-search`, « Dans les décisions » dans celui de `/norm-search`.
//!
//! Le fetch part en parallèle de la recherche principale, depuis un `Effect`
//! vers un signal. Zéro résultat ou erreur ⇒ le bloc disparaît. Le signal porte
//! la requête à laquelle il répond : tant que la requête courante n'est pas
//! celle-là, le bloc est en attente — il ne montre jamais la question
//! précédente.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{
    ArticleSearchHit, ArticleSearchResponse, SearchContext, SearchMode, SearchRequest,
    SearchResponse, SortOrder,
};

use crate::api::{ApiClient, PageParams, TextesFilters};
use crate::components::ui::Skeleton;
use crate::helpers::{encode_query, format_article_num};
use crate::jurisdiction::{home_place, use_reader_scope};

use super::compact_search::highlight::Highlighted;
use super::text_results::article_meta_line;

/// Hits affichés par teaser.
const TEASER_ROWS: usize = 3;

/// "Dans les textes" block of the decision search rail: top 3 articles of the text
/// engine for the same query under the same jurisdiction, and a link to its page.
#[component]
pub fn TextesTeaser(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let title = ui.in_texts;
    let answer = RwSignal::new(None::<(String, Option<ArticleSearchResponse>)>);
    let scope = use_reader_scope();
    Effect::new(move |_| {
        let q = query.get();
        if q.is_empty() {
            answer.set(None);
            return;
        }
        let places = vec![home_place(&scope.code())];
        leptos::task::spawn_local(async move {
            let resp = ApiClient::from_context()
                .search_textes(
                    &q,
                    TextesFilters {
                        places: &places,
                        ..Default::default()
                    },
                    PageParams {
                        limit: TEASER_ROWS as u32,
                        offset: 0,
                    },
                    // A teaser ranks a decisions query; it designates nothing.
                    lj_dtos::SearchMode::Lexical,
                    SearchContext::Teaser,
                )
                .await;
            if answer.is_disposed() || query.get_untracked() != q {
                return;
            }
            answer.set(Some((q, resp.ok())));
        });
    });

    move || {
        let q = query.get();
        if q.is_empty() {
            return ().into_any();
        }
        let Some(answered) = answer.get().filter(|(to, _)| *to == q) else {
            return teaser_skeleton(title);
        };
        let Some(resp) = answered.1 else {
            return ().into_any();
        };
        if resp.hits.is_empty() {
            return ().into_any();
        }
        let rows = resp
            .hits
            .into_iter()
            .take(TEASER_ROWS)
            .map(|h| view! { <TextTeaserRow hit=h /> })
            .collect_view();
        // No number, as on the bridge to the decisions: the exact count costs an
        // aggregate the bridge does not pay for three rows.
        teaser_block(
            title,
            rows.into_any(),
            scope.path(&format!("/norm-search?q={}", encode_query(&q))),
            ui.see_texts.to_string(),
        )
    }
}

/// "Dans les décisions" block of the norm search rail: top 3 decisions of the case
/// law engine for the same query under the same jurisdiction, and a link to its page.
#[component]
pub fn DecisionsTeaser(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let ui = &crate::i18n::use_ui().search;
    let title = ui.in_decisions;
    let answer = RwSignal::new(None::<(String, Option<SearchResponse>)>);
    let scope = use_reader_scope();
    let lang = crate::i18n::use_reader_lang().get();
    Effect::new(move |_| {
        let q = query.get();
        if q.is_empty() {
            answer.set(None);
            return;
        }
        let request = teaser_search_request(&q, scope.code(), lang.code());
        leptos::task::spawn_local(async move {
            let resp = ApiClient::from_context()
                .search(&request, SearchContext::Teaser)
                .await;
            if answer.is_disposed() || query.get_untracked() != q {
                return;
            }
            answer.set(Some((q, resp.ok())));
        });
    });

    move || {
        let q = query.get();
        if q.is_empty() {
            return ().into_any();
        }
        let Some(answered) = answer.get().filter(|(to, _)| *to == q) else {
            return teaser_skeleton(title);
        };
        let Some(resp) = answered.1 else {
            return ().into_any();
        };
        if resp.hits.is_empty() {
            return ().into_any();
        }
        let rows = resp
            .hits
            .into_iter()
            .take(TEASER_ROWS)
            .map(|h| {
                view! {
                    <DecisionTeaserRow
                        href=lj_dtos::decision_path(h.jurisdiction_type, h.jurisdiction_code.as_deref(), &h.id)
                        title_html=h.title_html
                        snippet=h.best_chunk.snippet
                    />
                }
            })
            .collect_view();
        // Sans nombre : le décompte exact vaut une seconde d'agrégat, que le pont
        // ne paie pas pour trois lignes de rail.
        teaser_block(
            title,
            rows.into_any(),
            scope.path(&format!("/decision-search?q={}", encode_query(&q))),
            ui.see_decisions.to_string(),
        )
    }
}

/// Ligne du teaser décisions : titre puis extrait du texte. L'extrait peut être
/// vide, et un nœud conditionnel parmi les enfants d'un `collect_view()` n'émet
/// son marqueur que dans une branche : la frontière de composant est ce qui le
/// rend hydratable.
#[component]
fn DecisionTeaserRow(href: String, title_html: String, snippet: String) -> impl IntoView {
    view! {
        <A href=href attr:class="group block py-1 no-underline">
            <span class="line-clamp-2 text-[13px] font-semibold leading-snug text-[var(--color-ink)] transition-colors group-hover:text-[var(--color-accent)]">
                <Highlighted text=title_html />
            </span>
            {Some(snippet)
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    view! {
                        <span class="mt-1 line-clamp-3 text-[12px] leading-snug text-[var(--color-ink-muted)]">
                            <Highlighted text=s />
                        </span>
                    }
                })}
        </A>
    }
}

/// Texts teaser row, laid out like a `/norm-search` hit.
#[component]
fn TextTeaserRow(mut hit: ArticleSearchHit) -> impl IntoView {
    crate::i18n::relabel_title(crate::i18n::use_reader_lang().get(), &mut hit.code_title);
    // `numKey` vide : hit « texte entier » → page du texte ; sinon page de
    // l'article.
    let is_whole_text = hit.num_key.is_empty();
    let text = lj_vocab::norm_path(&hit.jurisdiction, &hit.code);
    let href = if is_whole_text {
        text
    } else {
        format!("{text}/{}", hit.num_key)
    };
    let meta_line = article_meta_line(&hit);
    view! {
        <A href=href attr:class="group flex min-w-0 flex-col gap-1 py-1 no-underline">
            <span class="line-clamp-2 text-[13px] font-semibold leading-snug text-[var(--color-ink)] transition-colors group-hover:text-[var(--color-accent)]">
                {if is_whole_text {
                    view! { {hit.code_title.clone()} }.into_any()
                } else {
                    view! {
                        {(crate::i18n::use_ui().norm.article_n)(&format_article_num(&hit.num))}
                        <span class="font-normal text-[var(--color-ink-subtle)]">
                            {format!("\u{00A0}· {}", hit.code_title)}
                        </span>
                    }
                    .into_any()
                }}
            </span>
            {meta_line}
            {Some(hit.snippet)
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    view! {
                        <span class="line-clamp-3 text-[12px] leading-snug text-[var(--color-ink-muted)]">
                            <Highlighted text=s />
                        </span>
                    }
                })}
        </A>
    }
}

/// Requête décisions du teaser : top-3, tri pertinence, sans filtre ni mode IA.
/// Mode lexical forcé : le pont sert un extrait surligné, que produit la jambe
/// BM25.
fn teaser_search_request(q: &str, home: String, lang: &str) -> SearchRequest {
    SearchRequest {
        query: q.to_string(),
        jurisdiction_type: None,
        solution: None,
        legal_domain: None,
        jurisdiction_code: None,
        seat: None,
        home: Some(home),
        formation: None,
        legal_instrument: None,
        legal_article: None,
        publication: None,
        date_from: None,
        date_to: None,
        search_in: None,
        mode: SearchMode::Lexical,
        sort: SortOrder::Relevance,
        limit: TEASER_ROWS as u32,
        offset: 0,
        ai_mode: false,
        lang: Some(lang.to_owned()),
    }
}

/// Gabarit commun des deux blocs.
fn teaser_block(
    title: &'static str,
    rows: AnyView,
    more_href: String,
    more_label: String,
) -> AnyView {
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            {rows}
            <A
                href=more_href
                attr:class="pt-1 text-[13px] text-[var(--color-ink-muted)] underline-offset-2 transition-colors hover:text-[var(--color-accent)] hover:underline"
            >
                {more_label}
                " →"
            </A>
        </div>
    }
    .into_any()
}

/// Bloc en attente : le titre, qui ne dépend pas de la réponse, et des lignes
/// muettes à la place des hits de la question précédente.
fn teaser_skeleton(title: &'static str) -> AnyView {
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            <Skeleton class="h-4 w-full" />
            <Skeleton class="h-4 w-5/6" />
            <Skeleton class="h-4 w-2/3" />
            <Skeleton class="mt-1 h-4 w-32" />
        </div>
    }
    .into_any()
}
