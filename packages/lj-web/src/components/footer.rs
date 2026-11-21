use leptos::prelude::*;
use leptos_router::hooks::use_location;

const KO_FI: &str = "https://ko-fi.com/librejustice";

/// Bascule sur la charnière `lg` du site (`top_bar`, `layout`), sans palier.
/// Le repli s'équilibre sur la largeur : les espaces entre liens portent la
/// coupe et la gouttière, et un lien insécable ne se coupe pas en son milieu.
#[component]
pub fn Footer() -> impl IntoView {
    // Le signalement emporte la page d'où il part : depuis une décision, le
    // formulaire s'ouvre avec sa cible déjà remplie.
    let location = use_location();
    let ui = &crate::i18n::use_ui().shell;
    let lang = crate::i18n::use_reader_lang();
    let signaler = move || {
        lang.path(&format!(
            "/feedback?from={}",
            chemin_en_query(&location.pathname.get())
        ))
    };

    view! {
        <footer class="border-t border-[var(--color-rule)] bg-[var(--color-vellum)]">
            <div class="mx-auto flex max-w-7xl flex-col gap-4 px-4 py-6 text-sm text-[var(--color-ink-muted)] sm:px-6 lg:flex-row lg:items-center lg:justify-between lg:px-8 lg:py-8">
                <nav class="text-balance leading-7 [word-spacing:1rem] [&>a]:whitespace-nowrap [&>a]:[word-spacing:normal]">
                    <a class="hover:text-[var(--color-ink)]" href=lang.path("/sources")>
                        {ui.data}
                    </a>
                    " "
                    <a class="hover:text-[var(--color-ink)]" href=lang.path("/legal-notice")>
                        {ui.legal_notice}
                    </a>
                    " "
                    <a class="hover:text-[var(--color-ink)]" href=lang.path("/privacy")>
                        {ui.privacy}
                    </a>
                    " "
                    <a class="hover:text-[var(--color-ink)]" href=lang.path("/terms")>
                        {ui.terms}
                    </a>
                    " "
                    <a
                        class="hover:text-[var(--color-ink)]"
                        href="https://github.com/librejustice/librejustice"
                        target="_blank"
                        rel="noopener noreferrer"
                    >
                        {ui.source_code}
                    </a>
                    " "
                    <a
                        class="hover:text-[var(--color-ink)]"
                        href=KO_FI
                        target="_blank"
                        rel="noopener noreferrer"
                    >
                        {ui.support}
                    </a>
                    " "
                    <a class="hover:text-[var(--color-ink)]" href=signaler>
                        {ui.report_problem}
                    </a>
                </nav>
                <p class="hidden text-base lg:block">
                    <span
                        class="font-sans tracking-[0.02em] text-[var(--color-ink-muted)]"
                        style="font-variation-settings: 'wght' 300"
                    >
                        "Libre"
                    </span>
                    <span
                        class="font-sans tracking-[-0.02em] text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 650"
                    >
                        "Justice"
                    </span>
                </p>
            </div>
        </footer>
    }
}

/// Échappe un chemin pour qu'il traverse une query sans la couper.
fn chemin_en_query(chemin: &str) -> String {
    chemin
        .bytes()
        .map(|octet| match octet {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (octet as char).to_string()
            }
            _ => format!("%{octet:02X}"),
        })
        .collect()
}
