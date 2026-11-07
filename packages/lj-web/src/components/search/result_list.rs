//! `ResultList` : liste ordonnée de `ResultCard`.

use leptos::prelude::*;
use lj_dtos::SearchHit;

use super::result_card::ResultCard;
use super::view_prefs::CardView;

#[component]
pub fn ResultList(
    hits: Vec<SearchHit>,
    all_hit_paths: Vec<String>,
    /// The query whose terms the opened decision highlights.
    highlight: Option<String>,
    page: u32,
    page_size: u32,
    #[prop(into)] default_view: Signal<CardView>,
    animate: bool,
) -> impl IntoView {
    let all_hit_paths = std::sync::Arc::new(all_hit_paths);
    // Liste keyée par `hit.id` : quand les hits diffèrent (nouvelle requête, filtre,
    // page), les clés changent, `<For>` recrée les `<article>` et l'animation « rise »
    // rejoue. Mode IA (mêmes hits, mêmes clés) → nœuds réutilisés, pas de rejeu ; le
    // « mouvement » à l'activation IA vient des résumés.
    let hits: Vec<(usize, SearchHit)> = hits.into_iter().enumerate().collect();
    view! {
        <ol class="flex flex-col">
            <For each=move || hits.clone() key=|(_, hit)| hit.id.clone() let((index, hit))>
                <li class="contents">
                    <ResultCard
                        hit=hit
                        index=index
                        page=page
                        page_size=page_size
                        hit_paths=all_hit_paths.clone()
                        highlight=highlight.clone()
                        default_view=default_view
                        animate=animate
                    />
                </li>
            </For>
        </ol>
    }
}
