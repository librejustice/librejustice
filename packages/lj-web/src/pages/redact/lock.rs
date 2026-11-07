#![cfg(feature = "hydrate")]

use lj_vocab::files::FETCHED;

/// Bar the page from every request but the workbench's files, taken from `files`
/// or the page's own origin, and the page's `calls`. A policy only ever narrows
/// a page: nothing but a reload lifts it.
pub fn lock(files: Option<&str>, calls: &[&str]) {
    let window = web_sys::window().expect("the workbench runs in a window");
    let document = window.document().expect("the window has a document");
    let here = window.location().origin().expect("the page has an origin");
    let (origin, sources, workers) = match files {
        None => (here.as_str(), "'self'".to_string(), "'self'".to_string()),
        // A worker starts from a blob there, and its module imports answer to worker-src.
        Some(origin) => (origin, origin.to_string(), format!("blob: {origin}")),
    };
    let reach = FETCHED
        .iter()
        .map(|folder| format!("{origin}{folder}"))
        .chain(calls.iter().map(|call| format!("{here}{call}")))
        .collect::<Vec<_>>()
        .join(" ");
    let policy = format!(
        "default-src 'none'; script-src {sources} 'wasm-unsafe-eval'; style-src {sources} 'unsafe-inline'; \
         font-src {sources}; img-src data:; worker-src {workers}; form-action 'none'; connect-src {reach}"
    );
    let head = document
        .query_selector("head")
        .ok()
        .flatten()
        .expect("the page has a head");
    let meta = document.create_element("meta").expect("a meta element");
    meta.set_attribute("http-equiv", "Content-Security-Policy")
        .and_then(|()| meta.set_attribute("content", &policy))
        .and_then(|()| head.append_child(&meta))
        .expect("the workbench locks its page");
}
