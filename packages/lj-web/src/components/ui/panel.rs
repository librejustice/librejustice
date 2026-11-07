#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PanelEdge {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelAnchor {
    top: f64,
    edge: PanelEdge,
    offset: f64,
    min_width: f64,
    max_height: f64,
}

impl PanelAnchor {
    #[cfg(feature = "hydrate")]
    pub fn measure(button: &web_sys::Element, edge: PanelEdge, cap: f64) -> Self {
        let rect = button.get_bounding_client_rect();
        let dimension = |v: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
            v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
        };
        let window = leptos::prelude::window();
        let win_h = dimension(window.inner_height());
        let win_w = dimension(window.inner_width());
        let space_below = win_h - rect.bottom() - 8.0;
        let space_above = rect.top() - 8.0;
        let (top, max_height) = if space_below >= cap {
            (rect.bottom() + 4.0, cap)
        } else if space_above > space_below {
            let h = space_above.min(cap);
            (rect.top() - 4.0 - h, h)
        } else {
            (rect.bottom() + 4.0, space_below.max(0.0).min(cap))
        };
        let offset = match edge {
            PanelEdge::Left => rect.left(),
            PanelEdge::Right => win_w - rect.right(),
        };
        Self {
            top,
            edge,
            offset,
            min_width: rect.width(),
            max_height,
        }
    }

    pub fn style(anchor: Option<Self>) -> String {
        match anchor {
            Some(Self {
                top,
                edge,
                offset,
                min_width,
                max_height,
            }) => {
                let side = match edge {
                    PanelEdge::Left => "left",
                    PanelEdge::Right => "right",
                };
                format!(
                    "position:fixed; top:{top}px; {side}:{offset}px; width:max-content; min-width:{min_width}px; max-height:{max_height}px;"
                )
            }
            None => "position:fixed; top:0; left:0;".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Dismiss {
    pub escape: bool,
    pub scroll: bool,
}

#[cfg(feature = "hydrate")]
pub fn dismiss<El, M>(
    target: El,
    is_open: impl Fn() -> bool + Clone + Send + Sync + 'static,
    close: impl Fn() + Clone + Send + Sync + 'static,
    on: Dismiss,
) where
    El: leptos_use::core::IntoElementMaybeSignal<web_sys::EventTarget, M>,
{
    use leptos::prelude::GetUntracked;
    use leptos_use::{use_event_listener, use_window};
    let guarded = move || {
        if is_open() {
            close();
        }
    };
    // Not `on_click_outside`: on iOS, leptos-use 0.19 calls `new Function`,
    // which the CSP refuses, and hydration dies.
    let target = target.into_element_maybe_signal();
    let _ = use_event_listener(use_window(), leptos::ev::pointerdown, {
        let guarded = guarded.clone();
        move |ev| {
            if target
                .get_untracked()
                .is_some_and(|el| !ev.composed_path().includes(&el, 0))
            {
                guarded();
            }
        }
    });
    if on.escape {
        let guarded = guarded.clone();
        let _ = use_event_listener(use_window(), leptos::ev::keydown, move |ev| {
            if ev.key() == "Escape" {
                guarded();
            }
        });
    }
    if on.scroll {
        let _ = use_event_listener(use_window(), leptos::ev::scroll, move |_| guarded());
    }
}
