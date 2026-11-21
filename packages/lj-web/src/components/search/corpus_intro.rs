//! Ponts croisés des pages de recherche sans question posée : l'emplacement qui
//! porte la MÊME question à l'autre moteur montre alors le fonds d'en face.
//!
//! Fetch depuis un `Effect` vers un signal : jamais de `Resource` lue dans le
//! rendu d'un rail, et une erreur replie le bloc.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::CorpusStatsResponse;

use crate::api::ApiClient;
use crate::helpers::group_thousands;
use crate::jurisdiction::use_reader_scope;

/// Compteurs corpus, chargés une fois par page (cache 12 h côté API). Erreur ⇒
/// `None` : les blocs qui en dépendent disparaissent.
pub fn corpus_stats() -> Signal<Option<CorpusStatsResponse>> {
    let data = RwSignal::new(None::<CorpusStatsResponse>);
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            data.set(ApiClient::from_context().fetch_corpus_stats().await.ok());
        });
    });
    data.into()
}

/// « Dans les textes » bridge of the decision search without a query.
#[component]
pub fn TextesCorpusTeaser(
    #[prop(into)] stats: Signal<Option<CorpusStatsResponse>>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let scope = use_reader_scope();
    let ui = &crate::i18n::use_ui().search;
    move || {
        stats.get().map(|s| {
            rail_block(
                ui.in_texts,
                view! {
                    <p class="text-[13px] leading-snug text-[var(--color-ink-muted)]">
                        {(ui.texts_stats)(
                            &group_thousands(numbers, s.texts_count),
                            &group_thousands(numbers, s.articles_count),
                        )}
                    </p>
                }
                .into_any(),
                scope.path("/norm-search"),
                ui.search_texts.to_string(),
            )
        })
    }
}

/// Pont « Dans les décisions » de `/norm-search` sans requête.
#[component]
pub fn DecisionsCorpusTeaser(
    #[prop(into)] stats: Signal<Option<CorpusStatsResponse>>,
) -> impl IntoView {
    let numbers = &crate::i18n::use_ui().numbers;
    let scope = use_reader_scope();
    let ui = &crate::i18n::use_ui().search;
    move || {
        stats.get().map(|s| {
            rail_block(
                ui.in_decisions,
                view! {
                    <p class="text-[13px] leading-snug text-[var(--color-ink-muted)]">
                        {(ui.decisions_stats)(&group_thousands(numbers, s.decisions_count))}
                    </p>
                }
                .into_any(),
                scope.path("/decision-search"),
                ui.search_decisions.to_string(),
            )
        })
    }
}

/// Bloc de rail : filet haut, titre en petites capitales, contenu, lien « voir
/// tout ». Même anatomie que les blocs de facettes.
fn rail_block(
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
