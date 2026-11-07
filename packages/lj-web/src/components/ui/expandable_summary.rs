use leptos::html;
use leptos::prelude::*;

/// `clamp`: the line-clamp class the excerpt wears while folded.
#[component]
pub fn ExpandableSummary(
    excerpt: String,
    full: String,
    #[prop(optional)] clamp: &'static str,
    /// Hidden while folded, it unrolls above the text on unfolding.
    #[prop(default = None)]
    lead: Option<String>,
) -> impl IntoView {
    let unfolded = RwSignal::new(false);
    let shows_full = RwSignal::new(false);
    let node = NodeRef::<html::Button>::new();
    let lead_node = NodeRef::<html::Span>::new();
    #[cfg(feature = "hydrate")]
    let motion = motion::Motion::new(node, lead_node, shows_full);

    let toggle = move |_| {
        let opening = !unfolded.get_untracked();
        unfolded.set(opening);
        #[cfg(feature = "hydrate")]
        if motion.toggle(opening) {
            return;
        }
        shows_full.set(opening);
    };

    let lead_view = lead.map(|lead| {
        view! {
            <span
                node_ref=lead_node
                class=move || if shows_full.get() { "block overflow-hidden" } else { "hidden" }
            >
                {lead}
            </span>
        }
    });

    view! {
        <button
            type="button"
            node_ref=node
            aria-expanded=move || unfolded.get().to_string()
            title=move || if unfolded.get() { "Replier le résumé" } else { "Lire le résumé entier" }
            on:click=toggle
            class="block w-full cursor-pointer overflow-hidden text-left text-sm leading-snug text-[var(--color-ink-muted)] transition-colors hover:text-[var(--color-ink)]"
        >
            <span class=move || if shows_full.get() || clamp.is_empty() { "block" } else { clamp }>
                {lead_view}
                {move || if shows_full.get() { full.clone() } else { excerpt.clone() }}
            </span>
        </button>
    }
}

#[cfg(feature = "hydrate")]
mod motion {
    use std::rc::Rc;

    use leptos::html;
    use leptos::prelude::*;
    use web_sys::{CssStyleDeclaration, HtmlElement};

    const UNFOLD_MS: f64 = 220.0;

    /// The box's height and the lead's, run frame by frame between measured states.
    #[derive(Clone, Copy)]
    pub(super) struct Motion {
        node: NodeRef<html::Button>,
        lead: NodeRef<html::Span>,
        shows_full: RwSignal<bool>,
        folded_height: StoredValue<i32>,
        lead_opacity: StoredValue<f64>,
        run: StoredValue<u32>,
    }

    impl Motion {
        pub(super) fn new(
            node: NodeRef<html::Button>,
            lead: NodeRef<html::Span>,
            shows_full: RwSignal<bool>,
        ) -> Self {
            Self {
                node,
                lead,
                shows_full,
                folded_height: StoredValue::new(0),
                lead_opacity: StoredValue::new(0.0),
                run: StoredValue::new(0),
            }
        }

        /// Starts the fold or the unfold, false when the box is not mounted.
        pub(super) fn toggle(self, opening: bool) -> bool {
            let Some(el) = self.node.get_untracked() else {
                return false;
            };
            let from = el.offset_height();
            let style = HtmlElement::style(&el);
            let _ = style.set_property("height", &format!("{from}px"));
            let lead = self.lead.get_untracked();
            let lead_style = lead.as_ref().map(|l| HtmlElement::style(l));
            if opening && !self.shows_full.get_untracked() {
                self.folded_height.set_value(from);
                lead_style.as_ref().inspect(|l| set_lead(l, 0, 0.0));
            }
            // Folding keeps the full text until the box has closed on it.
            self.shows_full.set(true);
            self.run.update_value(|r| *r += 1);
            let this_run = self.run.get_value();
            let opacity_from = self.lead_opacity.get_value();
            let opacity_to = if opening { 1.0 } else { 0.0 };
            request_animation_frame(move || {
                let lead_from = lead.as_ref().map_or(0, |l| l.offset_height());
                let (lead_to, to) = if opening {
                    let lead_to = lead.as_ref().map_or(0, |l| l.scroll_height());
                    (lead_to, el.scroll_height() - lead_from + lead_to)
                } else {
                    (0, self.folded_height.get_value())
                };
                let frame = move |t: f64| {
                    if self.run.get_value() != this_run {
                        return false;
                    }
                    let eased = 1.0 - (1.0 - t).powi(3);
                    let lerp = |a: i32, b: i32| f64::from(a) + f64::from(b - a) * eased;
                    // Whole pixels: at a fractional offset, each line below snaps to the pixel
                    // grid on its own and the line spacing wavers.
                    let _ = style.set_property("height", &format!("{}px", lerp(from, to).round()));
                    let opacity = opacity_from + (opacity_to - opacity_from) * eased;
                    self.lead_opacity.set_value(opacity);
                    let lead_height = lerp(lead_from, lead_to).round() as i32;
                    lead_style
                        .as_ref()
                        .inspect(|l| set_lead(l, lead_height, opacity));
                    if t < 1.0 {
                        return true;
                    }
                    if opening {
                        lead_style.as_ref().inspect(|l| {
                            let _ = l.remove_property("height");
                        });
                    } else {
                        self.shows_full.set(false);
                    }
                    let _ = style.remove_property("height");
                    false
                };
                tick(now(), reduced_motion(), Rc::new(frame));
            });
            true
        }
    }

    fn set_lead(style: &CssStyleDeclaration, height: i32, opacity: f64) {
        let _ = style.set_property("height", &format!("{height}px"));
        let _ = style.set_property("opacity", &opacity.to_string());
    }

    fn tick(start: f64, instant: bool, frame: Rc<dyn Fn(f64) -> bool>) {
        request_animation_frame(move || {
            let t = if instant {
                1.0
            } else {
                ((now() - start) / UNFOLD_MS).min(1.0)
            };
            if frame(t) {
                tick(start, instant, frame);
            }
        });
    }

    fn now() -> f64 {
        window().performance().map_or(0.0, |p| p.now())
    }

    fn reduced_motion() -> bool {
        window()
            .match_media("(prefers-reduced-motion: reduce)")
            .ok()
            .flatten()
            .is_some_and(|m| m.matches())
    }
}
