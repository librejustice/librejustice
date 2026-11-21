use leptos::html;
use leptos::prelude::*;

/// `clamp`: the line-clamp class the excerpt wears while folded.
#[component]
pub fn ExpandableSummary(
    excerpt: String,
    full: String,
    #[prop(optional)] clamp: &'static str,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().decision;
    let unfolded = RwSignal::new(false);
    let shows_full = RwSignal::new(false);
    let node = NodeRef::<html::Button>::new();
    #[cfg(feature = "hydrate")]
    let motion = motion::Motion::new(node, shows_full);

    let toggle = move |_| {
        let opening = !unfolded.get_untracked();
        unfolded.set(opening);
        #[cfg(feature = "hydrate")]
        if motion.toggle(opening) {
            return;
        }
        shows_full.set(opening);
    };

    view! {
        <button
            type="button"
            node_ref=node
            aria-expanded=move || unfolded.get().to_string()
            title=move || if unfolded.get() { ui.fold_summary } else { ui.read_whole_summary }
            on:click=toggle
            class="block w-full cursor-pointer overflow-hidden text-left text-sm leading-snug text-[var(--color-ink-muted)] underline decoration-[var(--color-accent)]/30 underline-offset-[3px] transition-colors hover:text-[var(--color-ink)] hover:decoration-[var(--color-accent)]"
        >
            <span class=move || if shows_full.get() || clamp.is_empty() { "block" } else { clamp }>
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
    use web_sys::HtmlElement;

    const UNFOLD_MS: f64 = 220.0;

    /// The box's height, run frame by frame between measured states.
    #[derive(Clone, Copy)]
    pub(super) struct Motion {
        node: NodeRef<html::Button>,
        shows_full: RwSignal<bool>,
        folded_height: StoredValue<i32>,
        run: StoredValue<u32>,
    }

    impl Motion {
        pub(super) fn new(node: NodeRef<html::Button>, shows_full: RwSignal<bool>) -> Self {
            Self {
                node,
                shows_full,
                folded_height: StoredValue::new(0),
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
            if opening && !self.shows_full.get_untracked() {
                self.folded_height.set_value(from);
            }
            // Folding keeps the full text until the box has closed on it.
            self.shows_full.set(true);
            self.run.update_value(|r| *r += 1);
            let this_run = self.run.get_value();
            request_animation_frame(move || {
                let to = if opening {
                    el.scroll_height()
                } else {
                    self.folded_height.get_value()
                };
                let frame = move |t: f64| {
                    if self.run.get_value() != this_run {
                        return false;
                    }
                    let eased = 1.0 - (1.0 - t).powi(3);
                    let height = f64::from(from) + f64::from(to - from) * eased;
                    // Whole pixels: at a fractional offset, each line below snaps to the pixel
                    // grid on its own and the line spacing wavers.
                    let _ = style.set_property("height", &format!("{}px", height.round()));
                    if t < 1.0 {
                        return true;
                    }
                    if !opening {
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
