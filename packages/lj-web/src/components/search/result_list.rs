//! `ResultList` (port de `result-list.tsx`). Liste ordonnée de `ResultCard`.

use leptos::prelude::*;
use lj_dtos::SearchHit;

use super::result_card::ResultCard;

#[component]
pub fn ResultList(
    hits: Vec<SearchHit>,
    all_hit_ids: Vec<String>,
    page: u32,
    total: i64,
    page_size: u32,
    auto_load_summary: bool,
    animate: bool,
) -> impl IntoView {
    let all_hit_ids = std::sync::Arc::new(all_hit_ids);
    // Liste keyée par `hit.id` (parité React `<li key={hit.id}>`) : à chaque
    // recherche dont les hits diffèrent (nouvelle requête, filtre, page), les clés
    // changent → `<For>` recrée les `<article>` → l'animation « rise » rejoue. Un
    // simple `.map()` laissait `<Transition>` réutiliser les nœuds en place (patch
    // d'attributs), si bien que l'animation CSS — déjà jouée — ne redémarrait
    // jamais. Mode IA (mêmes hits, mêmes clés) → nœuds réutilisés → pas de rejeu
    // (comme React) ; le « mouvement » à l'activation IA vient des résumés.
    let hits: Vec<(usize, SearchHit)> = hits.into_iter().enumerate().collect();
    view! {
        <ol class="flex flex-col">
            <For each=move || hits.clone() key=|(_, hit)| hit.id.clone() let((index, hit))>
                <li class="contents">
                    <ResultCard
                        hit=hit
                        index=index
                        page=page
                        total=total
                        page_size=page_size
                        hit_ids=all_hit_ids.clone()
                        auto_load_summary=auto_load_summary
                        animate=animate
                    />
                </li>
            </For>
        </ol>
    }
}
