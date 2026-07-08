//! Sommaire ancré de la décision (scroll-spy). Port de `decision-toc.tsx`.
//!
//! Rendu statique (SSR) : liens `#ancre`, première section active, rail + barre
//! à la hauteur d'init (10px). Sous `hydrate`, le spy est branché : un shim JS
//! (`toc_spy.js`) mesure le DOM, Rust résout via `resolve_scroll_spy` et pose
//! les signals qui pilotent le markup.
use leptos::prelude::*;
use lj_dtos::ChronologyEntry;

use crate::components::hover_preview::{HoverPreview, PreviewKind};
use crate::helpers::{cn, format_iso_date};
use crate::pages::decision_page::sections::TocEntry;

#[component]
pub fn DecisionToc(sections: Vec<TocEntry>, chronology: Vec<ChronologyEntry>) -> impl IntoView {
    let chronology_view = chronology_block(chronology);
    if sections.is_empty() {
        return view! {
            <nav
                aria-label="Sommaire de la décision"
                class="flex flex-col gap-6 lg:sticky lg:top-20 lg:self-start"
            >
                {chronology_view}
            </nav>
        }
        .into_any();
    }

    // Section active : init = première du sommaire. Barre : init 10px.
    let active = RwSignal::new(sections.first().map(|s| s.id.clone()));
    let progress_px = RwSignal::new(10.0_f64);
    // Pilote `transition-[height]` : vrai uniquement pendant un scroll programmé
    // (clic), faux en suivi 1:1 du scroll libre.
    let animate = RwSignal::new(false);

    let list_ref = NodeRef::<leptos::html::Ul>::new();
    let section_ids: Vec<String> = sections.iter().map(|s| s.id.clone()).collect();

    #[cfg(feature = "hydrate")]
    let on_click = spy::wire(list_ref, section_ids, active, progress_px, animate);
    #[cfg(not(feature = "hydrate"))]
    let on_click = {
        let _ = (&list_ref, &section_ids);
        move |_ev: leptos::ev::MouseEvent, _id: String| {}
    };
    let on_click = StoredValue::new_local(on_click);

    let items = sections
        .into_iter()
        .map(move |section| {
            let id = StoredValue::new(section.id.clone());
            let is_active = move || id.with_value(|id| active.get().as_deref() == Some(id.as_str()));
            let link_class = move || {
                cn([
                    "block rounded-r-md py-1.5 pl-8 pr-3 text-sm no-underline transition-colors",
                    if is_active() {
                        "bg-[var(--color-vellum)] text-[var(--color-accent)]"
                    } else {
                        "text-[var(--color-ink-muted)] hover:bg-[var(--color-vellum)]/60 hover:text-[var(--color-ink)]"
                    },
                ])
            };
            let href = format!("#{}", section.id);
            let aria_current = move || is_active().then_some("true");
            let click_id = section.id.clone();
            let on_anchor_click = move |ev: leptos::ev::MouseEvent| {
                on_click.with_value(|f| f(ev, click_id.clone()));
            };
            view! {
                <li>
                    <a
                        href=href
                        aria-current=aria_current
                        on:click=on_anchor_click
                        class=link_class
                    >
                        {section.title}
                    </a>
                </li>
            }
        })
        .collect_view();

    let bar_class = move || {
        cn([
            "absolute left-[7px] top-2 w-[2px] rounded-full bg-[var(--color-accent)]",
            if animate.get() {
                "transition-[height] duration-300 ease-out"
            } else {
                ""
            },
        ])
    };
    let bar_style = move || format!("height:{}px", progress_px.get());

    view! {
        <nav
            aria-label="Sommaire de la décision"
            class="flex flex-col gap-6 lg:sticky lg:top-20 lg:self-start"
        >
            <div class="flex flex-col gap-3">
                <p class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "Sommaire"
                </p>
                <div class="relative">
                    <div class="absolute left-[7px] top-2 bottom-2 w-[2px] rounded-full bg-[var(--color-rule)]" />
                    <div aria-hidden="true" class=bar_class style=bar_style />
                    <ul node_ref=list_ref class="flex flex-col">
                        {items}
                    </ul>
                </div>
            </div>
            {chronology_view}
        </nav>
    }
    .into_any()
}

/// Libellé de la nature d'un lien de chronologie (`decision_links.link_type`,
/// ADR 0161), lu entre l'étape qui le porte et la décision attaquée en
/// dessous : « CE — sur pourvoi contre — CAA ».
fn link_label(key: &str) -> Option<&'static str> {
    match key {
        "APPEL_DE" => Some("sur appel de"),
        "POURVOI_CONTRE" => Some("sur pourvoi contre"),
        "RENVOI_APRES_CASSATION" => Some("sur renvoi après cassation par"),
        _ => None,
    }
}

/// Chronologie de l'affaire (ADR 0169) sous le sommaire, du plus récent au
/// plus ancien : une étape par décision de la chaîne appel/pourvoi/renvoi,
/// posée en points sur un rail fin — le même idiome visuel que le rail du
/// scroll-spy juste au-dessus. La décision courante porte le point plein
/// accent (non cliquable) ; les autres naviguent et portent la hover card de
/// décision (ADR 0168). La nature du lien (appel / pourvoi / renvoi,
/// ADR 0161) s'affiche en italique entre les étapes. Vide ⇒ pas de bloc.
fn chronology_block(chronology: Vec<ChronologyEntry>) -> Option<impl IntoView> {
    (!chronology.is_empty()).then(|| {
        let steps = chronology
            .into_iter()
            .map(|entry| {
                let date = entry.date.as_deref().map(|d| {
                    let d = format_iso_date(Some(d));
                    view! {
                        <span class="block text-xs text-[var(--color-ink-subtle)]">{d}</span>
                    }
                });
                // Point sur le rail : centré sur le trait (rail à x=8px, point
                // 10px posé à x=3px), plein accent pour l'étape courante.
                let dot_class = if entry.current {
                    "absolute left-[3px] top-[5px] h-[10px] w-[10px] rounded-full border-2 border-[var(--color-accent)] bg-[var(--color-accent)]"
                } else {
                    "absolute left-[3px] top-[5px] h-[10px] w-[10px] rounded-full border-2 border-[var(--color-rule)] bg-[var(--color-parchment)]"
                };
                let body = if entry.current {
                    view! {
                        <span aria-current="page" class="block">
                            <span class="block text-sm font-medium leading-snug text-[var(--color-ink)]">
                                {entry.label}
                            </span>
                            {date}
                        </span>
                    }
                    .into_any()
                } else {
                    let href = format!("/decision/{}", entry.id);
                    view! {
                        <HoverPreview kind=PreviewKind::Decision {
                            id: entry.id,
                        }>
                            <a href=href class="group block no-underline">
                                <span class="block text-sm leading-snug text-[var(--color-accent)] group-hover:underline">
                                    {entry.label}
                                </span>
                                {date}
                            </a>
                        </HoverPreview>
                    }
                    .into_any()
                };
                let relation = entry.link.as_deref().and_then(link_label).map(|l| {
                    view! {
                        <span class="mt-1.5 block text-xs italic text-[var(--color-ink-subtle)]">
                            {l}
                        </span>
                    }
                });
                view! {
                    <li class="relative pl-7">
                        <span aria-hidden="true" class=dot_class />
                        {body}
                        {relation}
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="flex flex-col gap-3">
                <p class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "Chronologie de l'affaire"
                </p>
                <div class="relative">
                    <div
                        aria-hidden="true"
                        class="absolute bottom-2 left-[7px] top-2 w-[2px] rounded-full bg-[var(--color-rule)]"
                    />
                    <ol class="flex flex-col gap-4">{steps}</ol>
                </div>
            </div>
        }
    })
}

#[cfg(feature = "hydrate")]
mod spy {
    use leptos::prelude::*;
    use leptos::reactive::owner::LocalStorage;
    use wasm_bindgen::prelude::*;

    use crate::pages::decision_page::toc_spy::{
        resolve_scroll_spy, SpyMetric, ANCHOR_SCROLL_MARGIN_PX, SCROLL_SPY_OFFSET_PX,
    };

    #[wasm_bindgen(module = "/src/components/decision/toc_spy.js")]
    extern "C" {
        #[wasm_bindgen(js_name = "observeScrollSpy")]
        fn observe_scroll_spy(
            list_el: &JsValue,
            section_ids: Vec<JsValue>,
            marker_offset: f64,
            on_resolve: &Closure<dyn FnMut(f64, js_sys::Float64Array, js_sys::Float64Array, bool)>,
            on_animate_off: &Closure<dyn FnMut()>,
        ) -> JsValue;

        #[wasm_bindgen(js_name = "scrollToSection")]
        fn scroll_to_section(
            id: &str,
            list_el: &JsValue,
            section_ids: Vec<JsValue>,
            marker_offset: f64,
            anchor_margin: f64,
            handle: &JsValue,
            on_landing: &Closure<dyn FnMut(f64, js_sys::Float64Array, js_sys::Float64Array, bool)>,
        );
    }

    // Construit les `SpyMetric` à partir des `Float64Array` parallèles aux ids
    // connus, puis résout et pose `active`/`progress_px`. Les ids non mesurés
    // (NaN) sont filtrés. Barre bornée à 10px comme `Math.max(10, …)` côté React.
    fn apply(
        section_ids: &[String],
        active: RwSignal<Option<String>>,
        progress_px: RwSignal<f64>,
        marker_y: f64,
        anchor_tops: &js_sys::Float64Array,
        centers: &js_sys::Float64Array,
        at_bottom: bool,
    ) {
        let mut metrics: Vec<SpyMetric> = Vec::with_capacity(section_ids.len());
        for (i, id) in section_ids.iter().enumerate() {
            let anchor_top = anchor_tops.get_index(i as u32);
            let center = centers.get_index(i as u32);
            if anchor_top.is_nan() || center.is_nan() {
                continue;
            }
            metrics.push(SpyMetric {
                id: id.clone(),
                anchor_top,
                center,
            });
        }
        if let Some(result) = resolve_scroll_spy(&metrics, marker_y, at_bottom) {
            active.set(Some(result.id));
            progress_px.set(result.progress.max(10.0));
        }
    }

    fn ids_to_js(section_ids: &[String]) -> Vec<JsValue> {
        section_ids.iter().map(|id| JsValue::from_str(id)).collect()
    }

    /// Branche le spy (observateur scroll/resize) et renvoie le handler de clic
    /// d'une entrée. Les handles JS (non `Send`) vivent dans des slots
    /// `LocalStorage` ; l'observateur est déconnecté à chaque re-branchement.
    pub fn wire(
        list_ref: NodeRef<leptos::html::Ul>,
        section_ids: Vec<String>,
        active: RwSignal<Option<String>>,
        progress_px: RwSignal<f64>,
        animate: RwSignal<bool>,
    ) -> impl Fn(leptos::ev::MouseEvent, String) + 'static {
        type ResolveClosure =
            Closure<dyn FnMut(f64, js_sys::Float64Array, js_sys::Float64Array, bool)>;

        let disconnect_slot: StoredValue<Option<JsValue>, LocalStorage> =
            StoredValue::new_local(None);
        let resolve_slot: StoredValue<Option<ResolveClosure>, LocalStorage> =
            StoredValue::new_local(None);
        let animate_off_slot: StoredValue<Option<Closure<dyn FnMut()>>, LocalStorage> =
            StoredValue::new_local(None);
        // Handle de l'observateur courant : passé à `scrollToSection` pour geler
        // le même observateur pendant le scroll programmé.
        let handle_slot: StoredValue<JsValue, LocalStorage> = StoredValue::new_local(JsValue::NULL);

        let ids = StoredValue::new_local(section_ids);

        Effect::new(move |_| {
            if let Some(prev) = disconnect_slot.try_update_value(Option::take).flatten() {
                if let Ok(f) = prev.clone().dyn_into::<js_sys::Function>() {
                    let _ = f.call0(&JsValue::NULL);
                }
            }
            let Some(node) = list_ref.get() else {
                return;
            };
            let ids_vec = ids.get_value();
            let on_resolve: ResolveClosure =
                Closure::new(move |marker_y, anchor_tops, centers, at_bottom| {
                    apply(
                        &ids.with_value(|v| v.clone()),
                        active,
                        progress_px,
                        marker_y,
                        &anchor_tops,
                        &centers,
                        at_bottom,
                    );
                });
            let on_animate_off: Closure<dyn FnMut()> = Closure::new(move || animate.set(false));

            let node_js: JsValue = node.into();
            let handle = observe_scroll_spy(
                &node_js,
                ids_to_js(&ids_vec),
                SCROLL_SPY_OFFSET_PX,
                &on_resolve,
                &on_animate_off,
            );
            handle_slot.set_value(handle.clone());
            disconnect_slot.set_value(Some(handle));
            resolve_slot.set_value(Some(on_resolve));
            animate_off_slot.set_value(Some(on_animate_off));
        });

        move |ev: leptos::ev::MouseEvent, id: String| {
            ev.prevent_default();
            animate.set(true);
            // Repli si la cible n'est pas mesurable (le shim ne rappellera pas
            // `on_landing`) : on pose au moins la section active, comme React.
            active.set(Some(id.clone()));
            let Some(node) = list_ref.get() else {
                return;
            };
            let ids_vec = ids.get_value();
            let on_landing: ResolveClosure =
                Closure::new(move |marker_y, anchor_tops, centers, at_bottom| {
                    apply(
                        &ids.with_value(|v| v.clone()),
                        active,
                        progress_px,
                        marker_y,
                        &anchor_tops,
                        &centers,
                        at_bottom,
                    );
                });
            let node_js: JsValue = node.into();
            // `scroll_to_section` rappelle `on_landing` de façon synchrone (une
            // fois) avant de rendre la main : le closure peut être déposé sur la
            // pile et droppé à la sortie, pas besoin de le conserver.
            handle_slot.with_value(|handle| {
                scroll_to_section(
                    &id,
                    &node_js,
                    ids_to_js(&ids_vec),
                    SCROLL_SPY_OFFSET_PX,
                    ANCHOR_SCROLL_MARGIN_PX,
                    handle,
                    &on_landing,
                );
            });
        }
    }
}
