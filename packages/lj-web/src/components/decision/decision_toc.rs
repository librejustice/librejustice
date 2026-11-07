//! Sommaire ancré de la décision (scroll-spy).
//!
//! Rendu statique (SSR) : liens `#ancre`, première section active, rail + barre
//! à la hauteur d'init (10px). Sous `hydrate`, le spy est branché :
//! `toc_spy_dom` mesure le DOM, `resolve_scroll_spy` résout, et les signals
//! pilotent le markup.
use leptos::prelude::*;
use lj_dtos::{ChronologyEntry, UiAction};

use crate::components::hover_preview::{HoverPreview, PreviewKind};
use crate::helpers::{cn, format_iso_date};
use crate::pages::decision_page::sections::TocEntry;

#[component]
pub fn DecisionToc(
    sections: Vec<TocEntry>,
    chronology: Vec<ChronologyEntry>,
    /// Occurrences of the searched terms under each entry.
    #[prop(into)]
    hits: Signal<Vec<usize>>,
) -> impl IntoView {
    let chronology_view = chronology_block(chronology);
    if sections.is_empty() {
        return view! {
            <nav aria-label="Sommaire de la décision" class="flex flex-col gap-6">
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
        .enumerate()
        .map(move |(i, section)| {
            let id = StoredValue::new(section.id.clone());
            let is_active = move || id.with_value(|id| active.get().as_deref() == Some(id.as_str()));
            let link_class = move || {
                cn([
                    "flex items-baseline justify-between gap-2 rounded-r-md py-1.5 pl-8 pr-3 text-sm no-underline transition-colors",
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
            let usage = section.usage.as_str();
            let on_anchor_click = move |ev: leptos::ev::MouseEvent| {
                on_click.with_value(|f| f(ev, click_id.clone()));
            };
            let count = move || {
                hits.with(|h| h.get(i).copied()).filter(|&n| n > 0).map(|n| {
                    let title = if n == 1 {
                        "1 occurrence des termes recherchés".to_string()
                    } else {
                        format!("{n} occurrences des termes recherchés")
                    };
                    let label = title.clone();
                    view! {
                        <span
                            title=title
                            class="shrink-0 rounded-full bg-[var(--color-accent-soft)] px-1.5 text-xs tabular-nums text-[var(--color-ink-muted)]"
                        >
                            <span aria-hidden="true">{n}</span>
                            <span class="sr-only">{label}</span>
                        </span>
                    }
                })
            };
            view! {
                <li data-usage=usage>
                    <a
                        href=href
                        aria-current=aria_current
                        on:click=on_anchor_click
                        class=link_class
                    >
                        <span>{section.title}</span>
                        {count}
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

    // Le collage (`lg:sticky`) est porté par le conteneur du rail dans
    // `decision_page.rs` — actions + sommaire collent d'un seul bloc.
    view! {
        <nav aria-label="Sommaire de la décision" class="flex flex-col gap-6">
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

/// Libellé de la nature d'un lien de chronologie (`decision_links.link_type`),
/// lu entre l'étape qui le porte et la décision attaquée en dessous :
/// « CE — sur pourvoi contre — CAA ».
fn link_label(key: &str) -> Option<&'static str> {
    match key {
        "APPEL_DE" => Some("sur appel de"),
        "POURVOI_CONTRE" => Some("sur pourvoi contre"),
        "RENVOI_APRES_CASSATION" => Some("sur renvoi après cassation par"),
        _ => None,
    }
}

/// Chronologie de l'affaire sous le sommaire, du plus récent au plus ancien :
/// une étape par décision de la chaîne appel/pourvoi/renvoi, posée en points sur
/// un rail fin — le même idiome visuel que le rail du scroll-spy juste au-dessus.
/// La décision courante porte le point plein accent (non cliquable) ; les autres
/// naviguent et portent la hover card de décision. La nature du lien (appel /
/// pourvoi / renvoi) s'affiche en italique entre les étapes. Vide ⇒ pas de bloc.
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
                let body = match entry
                    .id
                    .zip(entry.jurisdiction_type)
                    .filter(|_| !entry.current)
                {
                    Some((id, jurisdiction_type)) => {
                        let href = lj_dtos::decision_path(
                            jurisdiction_type,
                            entry.jurisdiction_code.as_deref(),
                            &id,
                        );
                        view! {
                            <HoverPreview kind=PreviewKind::Decision { id }>
                                <a href=href class="group block no-underline">
                                    <span class="block text-sm leading-snug text-[var(--color-accent)] group-hover:underline">
                                        {entry.label}
                                    </span>
                                    {date}
                                </a>
                            </HoverPreview>
                        }
                        .into_any()
                    }
                    None if entry.current => view! {
                        <span aria-current="page" class="block">
                            <span class="block text-sm font-medium leading-snug text-[var(--color-ink)]">
                                {entry.label}
                            </span>
                            {date}
                        </span>
                    }
                    .into_any(),
                    None => view! {
                        <span class="block" title="Décision non disponible">
                            <span class="block text-sm leading-snug text-[var(--color-ink-muted)]">
                                {entry.label}
                            </span>
                            {date}
                        </span>
                    }
                    .into_any(),
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
            <div class="flex flex-col gap-3" data-usage=UiAction::DecisionChronology.as_str()>
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

    use crate::components::decision::toc_spy_dom::{
        observe_scroll_spy, scroll_to_section, SpyHandle,
    };
    use crate::pages::decision_page::toc_spy::{
        resolve_scroll_spy, SpyMetric, ANCHOR_SCROLL_MARGIN_PX, SCROLL_SPY_OFFSET_PX,
    };

    /// Résout et pose `active`/`progress_px`. Barre bornée à 10px.
    fn apply(
        active: RwSignal<Option<String>>,
        progress_px: RwSignal<f64>,
        marker_y: f64,
        metrics: Vec<SpyMetric>,
        at_bottom: bool,
    ) {
        if let Some(result) = resolve_scroll_spy(&metrics, marker_y, at_bottom) {
            active.set(Some(result.id));
            progress_px.set(result.progress.max(10.0));
        }
    }

    /// Branche le spy (observateur scroll/resize) et renvoie le handler de clic
    /// d'une entrée. L'observateur (non `Send`) vit dans un slot `LocalStorage` ;
    /// son `Drop` retire les écouteurs à chaque re-branchement.
    pub fn wire(
        list_ref: NodeRef<leptos::html::Ul>,
        section_ids: Vec<String>,
        active: RwSignal<Option<String>>,
        progress_px: RwSignal<f64>,
        animate: RwSignal<bool>,
    ) -> impl Fn(leptos::ev::MouseEvent, String) + 'static {
        let handle_slot: StoredValue<Option<SpyHandle>, LocalStorage> =
            StoredValue::new_local(None);
        let ids = StoredValue::new_local(section_ids);

        Effect::new(move |_| {
            handle_slot.try_update_value(Option::take);
            let Some(node) = list_ref.get() else {
                return;
            };
            let liste: web_sys::Element = node.clone().into();
            let handle = observe_scroll_spy(
                liste,
                ids.get_value(),
                SCROLL_SPY_OFFSET_PX,
                move |marker_y, metrics, at_bottom| {
                    apply(active, progress_px, marker_y, metrics, at_bottom)
                },
                move || animate.set(false),
            );
            handle_slot.set_value(handle);
        });

        move |ev: leptos::ev::MouseEvent, id: String| {
            ev.prevent_default();
            animate.set(true);
            // Repli si la cible n'est pas mesurable (`on_landing` ne sera pas
            // appelé) : on pose au moins la section active.
            active.set(Some(id.clone()));
            let Some(node) = list_ref.get() else {
                return;
            };
            let liste: web_sys::Element = node.clone().into();
            handle_slot.with_value(|handle| {
                scroll_to_section(
                    handle.as_ref(),
                    &id,
                    &liste,
                    &ids.get_value(),
                    SCROLL_SPY_OFFSET_PX,
                    ANCHOR_SCROLL_MARGIN_PX,
                    |marker_y, metrics, at_bottom| {
                        apply(active, progress_px, marker_y, metrics, at_bottom)
                    },
                );
            });
        }
    }
}
