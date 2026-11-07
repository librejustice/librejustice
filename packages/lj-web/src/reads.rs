//! Tells the server what a page shows: a page served from the CDN cache never
//! reaches it, so only the browser can count the reading.

#[cfg(feature = "hydrate")]
static DOCUMENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
#[cfg(feature = "hydrate")]
static LANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The address the browser loaded, taken before the router can move it.
#[cfg(feature = "hydrate")]
pub fn note_document() {
    if let Some(href) = web_sys::window().and_then(|w| w.location().href().ok()) {
        let _ = DOCUMENT.set(href);
    }
}

#[cfg(feature = "hydrate")]
pub fn report_landing() {
    let Some(href) = DOCUMENT.get() else {
        return;
    };
    let after_host = href.split_once("://").map_or(href.as_str(), |(_, rest)| {
        rest.find('/').map_or("/", |i| &rest[i..])
    });
    let address = after_host.split('#').next().unwrap_or_default();
    let (path, search) = address.split_once('?').unwrap_or((address, ""));
    let report = lj_dtos::LandingReport {
        path: path.to_string(),
        referrer: web_sys::window()
            .and_then(|w| w.document())
            .map(|d| d.referrer())
            .filter(|r| !r.is_empty()),
        utm_source: query_value(search, "utm_source"),
        utm_campaign: query_value(search, "utm_campaign"),
    };
    leptos::task::spawn_local(async move {
        let _ = crate::api::ApiClient::from_context()
            .report_landing(&report)
            .await;
    });
}

/// A landing is the loaded document itself, reported once: a search page
/// followed by a click on a hit is a navigation, and so is a return to it.
#[cfg(feature = "hydrate")]
pub fn report(kind: lj_dtos::ReadKind, document: &str) {
    use std::sync::atomic::Ordering;

    let Some(window) = web_sys::window() else {
        return;
    };
    let href = window.location().href().ok();
    let landing =
        href.is_some() && DOCUMENT.get() == href.as_ref() && !LANDED.swap(true, Ordering::Relaxed);
    let (referrer, utm_source) = match landing {
        true => (
            window
                .document()
                .map(|d| d.referrer())
                .filter(|r| !r.is_empty()),
            window
                .location()
                .search()
                .ok()
                .as_deref()
                .and_then(|search| query_value(search, "utm_source")),
        ),
        false => (None, None),
    };
    let report = lj_dtos::ReadReport {
        kind,
        reference: window.location().pathname().unwrap_or_default(),
        document: document.to_string(),
        landing,
        referrer,
        utm_source,
    };
    leptos::task::spawn_local(async move {
        let _ = crate::api::ApiClient::from_context()
            .report_read(&report)
            .await;
    });
}

/// An annuaire search, reported for the same reason as a reading.
#[cfg(feature = "hydrate")]
pub fn report_entity_search(query: String) {
    let report = lj_dtos::EntitySearchReport { query };
    leptos::task::spawn_local(async move {
        let _ = crate::api::ApiClient::from_context()
            .report_entity_search(&report)
            .await;
    });
}

/// Counts the front feature a click uses: the control, inside the nearest block marked `data-usage`.
/// A copy counts the field marked `data-usage-copy` the selection starts in.
#[cfg(feature = "hydrate")]
pub fn count_actions() {
    use wasm_bindgen::{closure::Closure, JsCast};

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let on_click = Closure::<dyn Fn(web_sys::Event)>::new(|ev: web_sys::Event| {
        if let Some(action) = ev
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|element| element.closest(CONTROLS).ok().flatten())
            .and_then(|control| control.closest("[data-usage]").ok().flatten())
            .and_then(|block| block.get_attribute("data-usage"))
        {
            send_action(&action);
        }
    });
    let on_copy = Closure::<dyn Fn(web_sys::Event)>::new(|_: web_sys::Event| {
        if let Some(action) = web_sys::window()
            .and_then(|w| w.get_selection().ok().flatten())
            .and_then(|selection| selection.anchor_node())
            .and_then(|node| match node.dyn_into::<web_sys::Element>() {
                Ok(element) => Some(element),
                Err(node) => node.parent_element(),
            })
            .and_then(|element| element.closest("[data-usage-copy]").ok().flatten())
            .and_then(|field| field.get_attribute("data-usage-copy"))
        {
            send_action(&action);
        }
    });
    for (event, listener) in [("click", &on_click), ("copy", &on_copy)] {
        let _ = document.add_event_listener_with_callback_and_bool(
            event,
            listener.as_ref().unchecked_ref(),
            true,
        );
    }
    on_click.forget();
    on_copy.forget();
}

/// A beacon outlives the navigation the click starts, which a fetch does not.
#[cfg(feature = "hydrate")]
fn send_action(action: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window
            .navigator()
            .send_beacon(&format!("/internal/actions/{action}"));
    }
}

#[cfg(feature = "hydrate")]
const CONTROLS: &str = "a[href], button, summary, input[type=checkbox], input[type=radio], \
    [role=button], [role=option], [role=switch], [role=menuitem], [role=tab]";

#[cfg(feature = "hydrate")]
fn query_value(search: &str, name: &str) -> Option<String> {
    let value = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))?;
    js_sys::decode_uri_component(&value.replace('+', " "))
        .ok()
        .map(String::from)
        .filter(|v| !v.is_empty())
}
