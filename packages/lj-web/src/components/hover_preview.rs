//! Hover card de prévisualisation des références : au survol d'un lien
//! d'article de code (`/norm/{juridiction}/{code}/{num}`) ou de décision
//! (`/decision/{id}`),
//! un panneau flottant montre le contenu pointé sans naviguer.
//!
//! PHRASING CONTENT uniquement (`<span>`) : les déclencheurs vivent dans des
//! `<p>` (corps de décision) — un `<div>` fermerait le paragraphe au parse
//! HTML. Panneau en `position: fixed` mesuré à l'ouverture, collé au lien via un
//! pont transparent de 6 px, basculé au-dessus quand la place manque. Îlot
//! client : fetch dès le `mouseenter`, ouverture différée (150 ms), fermeture
//! différée (150 ms, le pointeur peut rejoindre le panneau). Rien au SSR ni au
//! tactile (pas de `mouseenter`).

use leptos::prelude::*;
use lj_dtos::{DecisionPreview, LawArticleResponse};

use crate::components::ui::{Badge, BadgeTone};
use crate::helpers::format_iso_date;
use crate::pages::decision_page::labels::publication_badge;
use crate::pages::law_page::article_title;

/// Cible prévisualisable d'un lien de référence. `date` (article) = date de la
/// décision hôte : la carte montre la version en vigueur À CETTE DATE — celle
/// que le juge appliquait — pas celle d'aujourd'hui.
#[derive(Clone)]
pub enum PreviewKind {
    Article {
        juris: String,
        code: String,
        num: String,
        date: Option<String>,
        place: Option<String>,
    },
    Decision {
        id: String,
    },
}

/// What an internal `href` previews. `/norm/{jurisdiction}/{code}/{num}` is
/// an article, at `at_date` when one is given; `…/{num}/{date}` is the article
/// at the date the link carries, which wins over the host context;
/// `/decision/{jurisdiction}/{id}` is a decision. Other shapes, a bare
/// `/norm/{jurisdiction}/{code}` or a division among them, have no card.
pub fn preview_kind(href: &str, at_date: Option<&str>) -> Option<PreviewKind> {
    if let Some(rest) = href.strip_prefix("/decision/") {
        let (juris, id) = rest.split_once('/')?;
        return (!juris.is_empty() && !id.is_empty() && !id.contains('/'))
            .then(|| PreviewKind::Decision { id: id.to_string() });
    }
    let (path, query) = href.split_once('?').unwrap_or((href, ""));
    let place = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("place="))
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    let rest = path.strip_prefix("/norm/")?;
    let mut parts = rest.split('/');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(juris), Some(code), Some(num), None, None)
            if !juris.is_empty() && !code.is_empty() && !num.is_empty() =>
        {
            Some(PreviewKind::Article {
                juris: juris.to_string(),
                code: code.to_string(),
                num: num.to_string(),
                date: at_date.map(str::to_string),
                place,
            })
        }
        (Some(juris), Some(code), Some(num), Some(date), None)
            if !juris.is_empty() && !code.is_empty() && !num.is_empty() && is_iso_date(date) =>
        {
            Some(PreviewKind::Article {
                juris: juris.to_string(),
                code: code.to_string(),
                num: num.to_string(),
                date: Some(date.to_string()),
                place,
            })
        }
        _ => None,
    }
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Payload chargé (article boxé : le `LawArticleResponse` porte timeline et
/// contexte, bien plus large que le variant décision). Construit par le fetch,
/// donc jamais au SSR — seul `card_view` (les deux cibles) le consomme.
#[derive(Clone)]
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
enum PreviewData {
    Article(Box<LawArticleResponse>),
    Decision(DecisionPreview),
}

/// Enveloppe hover d'un lien de référence : les enfants (le lien) restent tels
/// quels au SSR ; le panneau n'existe qu'ouvert, côté client.
#[component]
pub fn HoverPreview(kind: PreviewKind, children: Children) -> impl IntoView {
    let open = RwSignal::new(false);
    // Style de position du panneau (fixed, mesuré à l'ouverture). Avant mesure
    // il est déjà hors du flux : monté mais fermé, un panneau `static` occuperait
    // sa place en ligne dans le `<p>` hôte — `visibility:hidden` masque sans
    // libérer la place.
    let style = RwSignal::new("position:fixed; top:0; left:0;".to_string());
    let data = RwSignal::new(None::<PreviewData>);
    let anchor = NodeRef::<leptos::html::Span>::new();
    // Monté dès le premier survol, puis gardé (masqué par `.lj-panel`) : c'est
    // ce qui permet d'animer la fermeture, un panneau démonté n'ayant plus de
    // frames à jouer. Posé au `mouseenter`, 150 ms avant l'ouverture : le nœud
    // existe donc à l'état fermé pendant une frame au moins, et la première carte
    // s'ouvre avec sa transition.
    let mounted = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    let (on_enter, on_leave) = {
        use leptos::leptos_dom::helpers::{set_timeout_with_handle, TimeoutHandle};
        use std::time::Duration;

        let kind = StoredValue::new(kind);
        let open_timer = StoredValue::new_local(None::<TimeoutHandle>);
        let close_timer = StoredValue::new_local(None::<TimeoutHandle>);
        // Fetch en vol : évite le double départ pendant le chargement ; remis à
        // faux sur échec (nouvelle tentative au survol suivant).
        let loading = StoredValue::new(false);

        // Fetch dès l'entrée, pas à l'ouverture : le payload arrive pendant le
        // délai d'intention, la carte s'ouvre pleine.
        let do_fetch = move || {
            if data.get_untracked().is_some() || loading.get_value() {
                return;
            }
            loading.set_value(true);
            let kind = kind.get_value();
            leptos::task::spawn_local(async move {
                let client = crate::api::client::ApiClient::from_context();
                let fetched = match &kind {
                    PreviewKind::Article {
                        juris,
                        code,
                        num,
                        date,
                        place,
                    } => client
                        .fetch_legi_article(code, num, date.as_deref(), place.as_deref(), juris)
                        .await
                        .ok()
                        .map(|a| PreviewData::Article(Box::new(a))),
                    PreviewKind::Decision { id } => client
                        .fetch_decision_preview(id)
                        .await
                        .ok()
                        .map(PreviewData::Decision),
                };
                match fetched {
                    Some(d) => data.set(Some(d)),
                    None => {
                        loading.set_value(false);
                        open.set(false);
                    }
                }
            });
        };

        let do_open = move || {
            if let Some(el) = anchor.get_untracked() {
                const PANEL_W: f64 = 384.0;
                let rect = el.get_bounding_client_rect();
                let win_w = window()
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let win_h = window()
                    .inner_height()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let left = rect.left().min(win_w - PANEL_W - 8.0).max(8.0);
                let below = win_h - rect.bottom();
                // Below the link when there is room, above otherwise, anchored by `bottom`.
                // The 6 px gap is a transparent padding inside the anchor, crossed at any speed.
                let pos = if below >= 320.0 || below >= rect.top() {
                    format!("top:{}px; padding-top:6px;", rect.bottom())
                } else {
                    format!("bottom:{}px; padding-bottom:6px;", win_h - rect.top())
                };
                style.set(format!("position:fixed; left:{left}px; {pos}"));
            }
            open.set(true);
        };

        let on_enter = move |_: leptos::ev::MouseEvent| {
            if let Some(h) = close_timer.get_value() {
                h.clear();
                close_timer.set_value(None);
            }
            mounted.set(true);
            do_fetch();
            if open.get_untracked() || open_timer.get_value().is_some() {
                return;
            }
            let handle = set_timeout_with_handle(
                move || {
                    open_timer.set_value(None);
                    do_open();
                },
                Duration::from_millis(150),
            )
            .ok();
            open_timer.set_value(handle);
        };
        let on_leave = move |_: leptos::ev::MouseEvent| {
            if let Some(h) = open_timer.get_value() {
                h.clear();
                open_timer.set_value(None);
            }
            if !open.get_untracked() {
                return;
            }
            let handle = set_timeout_with_handle(
                move || {
                    close_timer.set_value(None);
                    open.set(false);
                },
                Duration::from_millis(150),
            )
            .ok();
            close_timer.set_value(handle);
        };
        (on_enter, on_leave)
    };
    #[cfg(not(feature = "hydrate"))]
    let (on_enter, on_leave) = {
        let _ = kind;
        (
            move |_: leptos::ev::MouseEvent| {},
            move |_: leptos::ev::MouseEvent| {},
        )
    };

    view! {
        <span node_ref=anchor on:mouseenter=on_enter on:mouseleave=on_leave>
            {children()}
            <Show when=move || mounted.get()>
                // A DOM child of the anchor, covered by its handlers. Only the card inside
                // slides: a sliding bridge covered the link's lower edge, taking its hover.
                // `data-hover-card`: the body's selection toolbar ignores this island.
                <span
                    data-hover-card=""
                    data-open=move || open.get().then_some("true")
                    style=move || style.get()
                    class="pointer-events-none z-50 block w-96 max-w-[calc(100vw-16px)] data-open:pointer-events-auto"
                >
                    <span
                        data-open=move || open.get().then_some("true")
                        class="lj-panel block rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-4 text-left shadow-lg"
                    >
                        {move || card_view(data.get())}
                    </span>
                </span>
            </Show>
        </span>
    }
}

fn card_view(data: Option<PreviewData>) -> AnyView {
    match data {
        None => view! {
            <span class="block text-xs text-[var(--color-ink-subtle)]">"Chargement…"</span>
        }
        .into_any(),
        Some(PreviewData::Article(article)) => article_card(*article),
        Some(PreviewData::Decision(preview)) => decision_card(preview),
    }
}

/// Carte article : titre, période de validité de la version servie, texte
/// scrollable. Une version close (`date_fin`) — typiquement la version à la
/// date de la décision hôte — affiche sa plage ; la version courante, son
/// entrée en vigueur.
fn article_card(article: LawArticleResponse) -> AnyView {
    let title = article_title(&article);
    let since = match article.date_fin.as_deref() {
        Some(fin) => format!(
            "En vigueur du {} au {}",
            format_iso_date(Some(&article.date_debut)),
            format_iso_date(Some(fin))
        ),
        None => format!(
            "En vigueur depuis le {}",
            format_iso_date(Some(&article.date_debut))
        ),
    };
    let texte = article.texte.unwrap_or_default();
    view! {
        <span class="block font-sans text-sm font-semibold leading-snug text-[var(--color-ink)]">
            {title}
        </span>
        <span class="mt-0.5 block text-xs text-[var(--color-ink-subtle)]">{since}</span>
        <span class="mt-2 block max-h-56 overflow-y-auto overscroll-contain whitespace-pre-line text-xs leading-relaxed text-[var(--color-ink-muted)]">
            {texte}
        </span>
    }
    .into_any()
}

/// Decision card: title, badges (solution and reach, in the tones of the
/// result cards), scrollable summary.
fn decision_card(preview: DecisionPreview) -> AnyView {
    let solution = preview.solution.map(|s| s.label);
    let publication = publication_badge(&preview.publication_codes);
    let badges = (solution.is_some() || publication.is_some()).then(|| {
        view! {
            <span class="mt-1.5 flex flex-wrap gap-1.5">
                {solution.map(|label| view! { <Badge tone=BadgeTone::Outline>{label}</Badge> })}
                {publication.map(|label| view! { <Badge tone=BadgeTone::Neutral>{label}</Badge> })}
            </span>
        }
    });
    let summary = preview.summary.unwrap_or_default();
    view! {
        <span class="block font-sans text-sm font-semibold leading-snug text-[var(--color-ink)]">
            {preview.title}
        </span>
        {badges}
        <span class="mt-2 block max-h-56 overflow-y-auto overscroll-contain text-xs leading-relaxed text-[var(--color-ink-muted)]">
            {summary}
        </span>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_kind_parses_article_and_decision() {
        assert!(matches!(
            preview_kind("/norm/fr/code-civil/1728", Some("2015-06-04")),
            Some(PreviewKind::Article { juris, code, num, date, place: None })
                if juris == "fr" && code == "code-civil" && num == "1728"
                    && date.as_deref() == Some("2015-06-04")
        ));
        assert!(matches!(
            preview_kind("/norm/fr/code-civil/1728?place=NC", None),
            Some(PreviewKind::Article { num, place: Some(p), .. }) if num == "1728" && p == "NC"
        ));
        assert!(matches!(
            preview_kind("/norm/fr/code-civil/1728", None),
            Some(PreviewKind::Article { date: None, .. })
        ));
        assert!(matches!(
            preview_kind("/norm/fr/code-civil/1728/2016-10-01", Some("2015-06-04")),
            Some(PreviewKind::Article { date: Some(d), .. }) if d == "2016-10-01"
        ));
        assert!(matches!(
            preview_kind("/decision/fr/z-hk-uk5YqPg", None),
            Some(PreviewKind::Decision { id }) if id == "z-hk-uk5YqPg"
        ));
    }

    #[test]
    fn preview_kind_rejects_other_shapes() {
        assert!(preview_kind("/norm/fr/code-civil", None).is_none());
        assert!(preview_kind("/norm/fr/code-civil/1728/2016-10-01/x", None).is_none());
        assert!(preview_kind("/decision/", None).is_none());
        assert!(preview_kind("/decision/z-hk-uk5YqPg", None).is_none());
        assert!(preview_kind("/decision/fr/a/b", None).is_none());
        assert!(preview_kind("/norm/fr/code-civil/livre-i/titre-v", None).is_none());
        assert!(preview_kind("/decision-search", None).is_none());
    }
}
