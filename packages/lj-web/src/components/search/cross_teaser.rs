//! Ponts croisés entre les deux moteurs : bloc « Dans les textes » dans le
//! rail de `/decisions`, « Dans les décisions » dans celui de `/textes`. Un
//! utilisateur qui pose une question de normes dans le moteur décisions (ou
//! l'inverse) est rattrapé sur la page de résultats au lieu de relancer la
//! même requête en boucle (note working-notes 2026-07-22).
//!
//! Le fetch (limit 3) part en parallèle de la recherche principale, depuis un
//! `Effect` vers un signal — même doctrine que facettes/volumétrie du rail :
//! jamais de lecture de `Resource` dans le rendu du rail, bloc précédent
//! conservé pendant un refetch. Zéro résultat ou erreur ⇒ le bloc disparaît,
//! aucun état d'erreur (enrichissement optionnel, pas un contenu).

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{
    ArticleSearchResponse, SearchContext, SearchMode, SearchRequest, SearchResponse, SortOrder,
};

use crate::api::{ApiClient, PageParams, TextesFilters};
use crate::helpers::{encode_query, format_article_num, group_thousands};

use super::compact_search::highlight::Highlighted;

/// Hits affichés par teaser.
const TEASER_ROWS: usize = 3;

/// Bloc « Dans les textes » du rail `/decisions` : top-3 articles du moteur
/// textes pour la même requête + lien vers `/textes?q=…`.
#[component]
pub fn TextesTeaser(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let data = RwSignal::new(None::<(String, ArticleSearchResponse)>);
    Effect::new(move |_| {
        let q = query.get();
        if q.is_empty() {
            data.set(None);
            return;
        }
        leptos::task::spawn_local(async move {
            let resp = ApiClient::from_context()
                .search_textes(
                    &q,
                    TextesFilters::default(),
                    PageParams {
                        limit: TEASER_ROWS as u32,
                        offset: 0,
                    },
                    SearchContext::Teaser,
                )
                .await;
            // Réponse en retard (la requête a changé depuis) : ignorée.
            if query.get_untracked() != q {
                return;
            }
            data.set(resp.ok().map(|r| (q, r)));
        });
    });

    move || {
        let Some((q, resp)) = data.get() else {
            return ().into_any();
        };
        if resp.hits.is_empty() {
            return ().into_any();
        }
        let total = resp.total;
        let rows = resp
            .hits
            .into_iter()
            .take(TEASER_ROWS)
            .map(|h| {
                // Hit « texte entier » (`numKey` vide, texte à corps ADR 0196)
                // → page du texte ; sinon page de l'article.
                let (href, label) = if h.num_key.is_empty() {
                    (format!("/texte/{}", h.code), h.code_title)
                } else {
                    (
                        format!("/texte/{}/{}", h.code, h.num_key),
                        format!("Art. {} · {}", format_article_num(&h.num), h.code_title),
                    )
                };
                teaser_row(href, label)
            })
            .collect_view();
        let more_label = if total == 1 {
            "Voir le texte".to_string()
        } else {
            format!("Voir les {} textes", group_thousands(total))
        };
        teaser_block(
            "Dans les textes",
            rows.into_any(),
            format!("/textes?q={}", encode_query(&q)),
            more_label,
        )
    }
}

/// Bloc « Dans les décisions » du rail `/textes` : top-3 décisions du moteur
/// jurisprudence pour la même requête + lien vers `/decisions?q=…`.
#[component]
pub fn DecisionsTeaser(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let data = RwSignal::new(None::<(String, SearchResponse)>);
    Effect::new(move |_| {
        let q = query.get();
        if q.is_empty() {
            data.set(None);
            return;
        }
        leptos::task::spawn_local(async move {
            let request = teaser_search_request(&q);
            let resp = ApiClient::from_context()
                .search(&request, SearchContext::Teaser)
                .await;
            if query.get_untracked() != q {
                return;
            }
            data.set(resp.ok().map(|r| (q, r)));
        });
    });

    move || {
        let Some((q, resp)) = data.get() else {
            return ().into_any();
        };
        if resp.hits.is_empty() {
            return ().into_any();
        }
        let total = resp.total;
        let rows = resp
            .hits
            .into_iter()
            .take(TEASER_ROWS)
            .map(|h| {
                view! {
                    <A
                        href=format!("/decision/{}", h.id)
                        attr:class="block truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors hover:text-[var(--color-ink)]"
                    >
                        <Highlighted text=h.title_html />
                    </A>
                }
            })
            .collect_view();
        let more_label = if total == 1 {
            "Voir la décision".to_string()
        } else {
            format!("Voir les {} décisions", group_thousands(total))
        };
        teaser_block(
            "Dans les décisions",
            rows.into_any(),
            format!("/decisions?q={}", encode_query(&q)),
            more_label,
        )
    }
}

/// Requête décisions minimale du teaser : requête seule, top-3, tri
/// pertinence, sans filtre ni mode IA.
fn teaser_search_request(q: &str) -> SearchRequest {
    SearchRequest {
        query: q.to_string(),
        jurisdiction_type: None,
        solution: None,
        procedure: None,
        office: None,
        legal_domain: None,
        jurisdiction_code: None,
        chamber: None,
        legal_instrument: None,
        legal_article: None,
        significance: None,
        publication: None,
        date_from: None,
        date_to: None,
        mode: SearchMode::Auto,
        sort: SortOrder::Relevance,
        limit: TEASER_ROWS as u32,
        offset: 0,
        ai_mode: false,
    }
}

/// Gabarit commun des deux blocs (même anatomie que les blocs de facettes du
/// rail : filet haut, titre en petites capitales, lignes, lien « voir tout »).
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

/// Ligne d'un hit texte (label plein, tronqué à une ligne).
fn teaser_row(href: String, label: String) -> impl IntoView {
    view! {
        <A
            href=href
            attr:class="block truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors hover:text-[var(--color-ink)]"
        >
            {label}
        </A>
    }
}
