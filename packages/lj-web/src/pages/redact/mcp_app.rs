//! The workbench as an MCP App view, which loads the front from the site's origin.

#[cfg(feature = "hydrate")]
use leptos::prelude::*;

/// The view's language: a chat frames it at no address that would name one.
const LANG: crate::i18n::Lang = crate::i18n::Lang::Fr;

/// Hosts keep this document as long as they like, so it names no build: the
/// entry module it imports does.
#[cfg(feature = "ssr")]
pub fn document(origin: &str) -> String {
    let literal = |text: &str| serde_json::to_string(text).expect("a string serialises");
    let entry = literal(&format!("{origin}{}", lj_vocab::files::ENTRY));
    let origin = literal(origin);
    let ui = &LANG.ui().redact;
    let (lang, loading, failed) = (LANG.code(), ui.mcp_loading, literal(ui.mcp_failed));
    format!(
        r#"<!doctype html>
<html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"></head>
<body class="antialiased bg-[var(--color-parchment)] text-[var(--color-ink)]">
<p>{loading}</p>
<script type="module">
try {{
  const {{ mount }} = await import({entry});
  await mount({origin});
}} catch (error) {{
  document.body.textContent = {failed};
  throw error;
}}
</script>
</body></html>"#
    )
}

/// The module at [`lj_vocab::files::ENTRY`]: the running build's stylesheet and front,
/// then the workbench mounted.
#[cfg(feature = "ssr")]
pub fn entry(options: &leptos::config::LeptosOptions) -> String {
    let pkg = format!("/{}", options.site_pkg_dir);
    let [js, wasm, css] = bundle(options);
    format!(
        r#"import init, {{ mount_workbench }} from "{pkg}/{js}.js";

export async function mount(origin) {{
  const style = document.createElement("link");
  style.rel = "stylesheet";
  style.href = new URL("{pkg}/{css}.css", import.meta.url).href;
  const styled = new Promise((resolve) => {{ style.onload = style.onerror = resolve; }});
  document.head.append(style);
  await init({{ module_or_path: new URL("{pkg}/{wasm}.wasm", import.meta.url) }});
  await styled;
  mount_workbench(origin);
}}
"#
    )
}

/// The front's file names, as `HydrationScripts` and `HashedStylesheet` write them.
#[cfg(feature = "ssr")]
fn bundle(options: &leptos::config::LeptosOptions) -> [String; 3] {
    let hashes = options
        .hash_files
        .then(|| std::env::current_exe().ok())
        .flatten()
        .and_then(|exe| std::fs::read_to_string(exe.parent()?.join(&*options.hash_file)).ok())
        .unwrap_or_default();
    let named = |kind: &str| {
        let hash = hashes.lines().find_map(|line| {
            let (key, hash) = line.split_once(':')?;
            (key.trim() == kind).then(|| hash.trim())
        });
        match hash {
            Some(hash) => format!("{}.{hash}", options.output_name),
            None => options.output_name.to_string(),
        }
    };
    let wasm = match options.hash_files || option_env!("LEPTOS_OUTPUT_NAME").is_some() {
        true => named("wasm"),
        false => format!("{}_bg", options.output_name),
    };
    [named("js"), wasm, named("css")]
}

#[cfg(feature = "hydrate")]
pub fn mount(files: String) {
    if let Some(body) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.body())
    {
        body.set_text_content(None);
    }
    super::engine::fetch_files_from(files.clone());
    super::lock::lock(Some(&files), &[]);
    leptos::mount::mount_to_body(Card);
}

#[cfg(feature = "hydrate")]
#[component]
fn Card() -> impl IntoView {
    use super::host::{Host, Insets, Mode, Saved};
    use super::outlet::{Outlet, Outlets};
    use super::state::{Session, Status};

    crate::i18n::provide_lang(LANG);
    let ui = &crate::i18n::use_ui().redact;
    let host = Host::connect();
    send_links_to(host);
    let session = Session::new(ui);
    let mut outlets = Vec::new();
    if host.stores_files() {
        outlets.push(Outlet::new(ui.add_to_chat, move |name, bytes| async move {
            host.attach(&neutral(ui, &name), &bytes, ui.attach_prompt)
                .await?;
            Ok(Some(ui.sent_to_chat))
        }));
    }
    outlets.push(Outlet::new(ui.download, move |name, bytes| async move {
        Ok(host
            .download(&neutral(ui, &name), &bytes)
            .await?
            .map(|saved| match saved {
                Saved::Opened => ui.opens_in_tab,
                Saved::Downloaded => ui.downloaded,
            }))
    }));
    provide_context(Outlets(outlets));

    let full = move || host.mode.get() == Mode::Fullscreen;
    let height = move || match full() {
        true => "h-dvh",
        false => "h-[44rem]",
    };
    let padding = move || {
        let Insets {
            top,
            right,
            bottom,
            left,
        } = host.insets.get();
        let [top, right, bottom, left] = [top, right, bottom, left].map(|side| side + 8.0);
        format!("padding: {top}px {right}px {bottom}px {left}px")
    };
    let title = move || match session.status.get() {
        Status::Ready => view! { <super::Toolbar session=session /> }.into_any(),
        _ => view! {
            <p class="min-w-0 flex-1 truncate font-sans text-sm font-medium">{ui.workbench}</p>
        }
        .into_any(),
    };
    let toggle = move |_| {
        host.show(match full() {
            true => Mode::Inline,
            false => Mode::Fullscreen,
        })
    };

    view! {
        <div class=move || format!("flex flex-col gap-2 {}", height()) style=padding>
            <header class="flex h-12 shrink-0 items-center gap-2">
                {title}
                <Show when=move || host.expands.get()>
                    <button type="button" class=super::SECONDARY on:click=toggle>
                        {move || match full() {
                            true => ui.collapse,
                            false => ui.expand,
                        }}
                    </button>
                </Show>
            </header>
            <super::RedactTool session=session />
        </div>
    }
}

/// What the view hands the chat is named after none of the original's name,
/// which often carries a party's.
#[cfg(feature = "hydrate")]
fn neutral(ui: &crate::i18n::RedactUi, name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, extension)) => format!("{}.{extension}", ui.neutral_name),
        None => ui.neutral_name.to_string(),
    }
}

/// A sandboxed view navigates nowhere: the host opens its links.
#[cfg(feature = "hydrate")]
fn send_links_to(host: super::host::Host) {
    use wasm_bindgen::JsCast;
    let _ = window_event_listener(leptos::ev::click, move |event| {
        let link = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|element| element.closest("a[href]").ok().flatten())
            .and_then(|element| element.dyn_into::<web_sys::HtmlAnchorElement>().ok());
        if let Some(link) = link.filter(|link| link.href().starts_with("http")) {
            event.prevent_default();
            host.open_link(link.href());
        }
    });
}
