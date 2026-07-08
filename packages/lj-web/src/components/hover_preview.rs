//! Hover card de prévisualisation des références (ADR 0168) : au survol d'un
//! lien d'article de code (`/loi/{code}/{num}`) ou de décision
//! (`/decision/{id}`), un panneau flottant montre le contenu pointé sans
//! naviguer — gabarit  (recon 2026-07-02, shots 14/15).
//!
//! PHRASING CONTENT uniquement (`<span>`) : les déclencheurs vivent dans des
//! `<p>` (corps de décision) — un `<div>` fermerait le paragraphe au parse
//! HTML (même contrainte que `CiteMenu`). Panneau en `position: fixed` mesuré
//! à l'ouverture (même doctrine que `FilterDropdown`), collé au lien via un
//! pont transparent de 6 px (pas de zone morte à traverser), basculé
//! au-dessus quand la place manque. Îlot client : fetch dès le `mouseenter`,
//! ouverture différée (150 ms), fermeture différée (150 ms — le pointeur peut
//! rejoindre le panneau) ; endpoints déjà cachés CDN. Rien au SSR ni au
//! tactile (pas de `mouseenter`).

use leptos::prelude::*;
use lj_dtos::{DecisionPreview, LawArticleResponse};

use crate::components::ui::{Badge, BadgeTone};
use crate::helpers::format_iso_date;
use crate::pages::decision_page::labels::portee_badge;
use crate::pages::law_page::article_title;

/// Cible prévisualisable d'un lien de référence. `date` (article) = date de la
/// décision hôte : la carte montre la version en vigueur À CETTE DATE — celle
/// que le juge appliquait — pas celle d'aujourd'hui.
#[derive(Clone)]
pub enum PreviewKind {
    Article {
        code: String,
        num: String,
        date: Option<String>,
    },
    Decision {
        id: String,
    },
}

/// Cible de prévisualisation d'un `href` interne : `/loi/{code}/{num}` →
/// article (version à `at_date` si fournie), `/decision/{id}` → décision. Les
/// autres formes (`/loi/{code}` nu, version datée `/loi/{code}/{num}/{date}`)
/// n'ont pas de carte.
pub fn preview_kind(href: &str, at_date: Option<&str>) -> Option<PreviewKind> {
    if let Some(id) = href.strip_prefix("/decision/") {
        return (!id.is_empty() && !id.contains('/'))
            .then(|| PreviewKind::Decision { id: id.to_string() });
    }
    let rest = href.strip_prefix("/loi/")?;
    let mut parts = rest.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(code), Some(num), None) if !code.is_empty() && !num.is_empty() => {
            Some(PreviewKind::Article {
                code: code.to_string(),
                num: num.to_string(),
                date: at_date.map(str::to_string),
            })
        }
        _ => None,
    }
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
    // Style de position du panneau (fixed, mesuré à l'ouverture).
    let style = RwSignal::new(String::new());
    let data = RwSignal::new(None::<PreviewData>);
    let anchor = NodeRef::<leptos::html::Span>::new();

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

        // Fetch dès l'ENTRÉE (pas à l'ouverture) : le payload arrive pendant le
        // délai d'intention — la carte s'ouvre pleine, sans flash « Chargement… ».
        let do_fetch = move || {
            if data.get_untracked().is_some() || loading.get_value() {
                return;
            }
            loading.set_value(true);
            let kind = kind.get_value();
            leptos::task::spawn_local(async move {
                let client = crate::api::client::ApiClient::from_context();
                let fetched = match &kind {
                    PreviewKind::Article { code, num, date } => client
                        .fetch_legi_article(code, num, date.as_deref())
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
                // Sous le lien si la place suffit (ou dépasse celle du dessus),
                // au-dessus sinon (ancrage `bottom`, le panneau pousse vers le
                // haut). Le wrapper est COLLÉ au lien : l'écart visuel de 6 px
                // est un padding transparent qui porte les handlers — pas de
                // zone morte entre lien et panneau, le pointeur peut la
                // traverser à n'importe quelle vitesse.
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
            <Show when=move || open.get()>
                // Le wrapper (pont transparent inclus) garde les handlers : y
                // entrer annule la fermeture différée (contenu scrollable
                // atteignable au pointeur).
                <span
                    style=move || style.get()
                    on:mouseenter=on_enter
                    on:mouseleave=on_leave
                    class="z-50 block w-96 max-w-[calc(100vw-16px)]"
                >
                    <span class="block rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-4 text-left shadow-lg">
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

/// Carte décision : titre, badges (solution + voie + portée, mêmes tons que
/// les cartes résultat), résumé scrollable.
fn decision_card(preview: DecisionPreview) -> AnyView {
    let solution = preview.solution.map(|s| s.label);
    let voie = preview.voie.map(|v| v.label);
    let portee = portee_badge(&preview.publication_codes);
    let badges = (solution.is_some() || voie.is_some() || portee.is_some()).then(|| {
        view! {
            <span class="mt-1.5 flex flex-wrap gap-1.5">
                {solution.map(|label| view! { <Badge tone=BadgeTone::Outline>{label}</Badge> })}
                {voie.map(|label| view! { <Badge tone=BadgeTone::Accent>{label}</Badge> })}
                {portee.map(|label| view! { <Badge tone=BadgeTone::Neutral>{label}</Badge> })}
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
            preview_kind("/loi/code-civil/1728", Some("2015-06-04")),
            Some(PreviewKind::Article { code, num, date })
                if code == "code-civil" && num == "1728" && date.as_deref() == Some("2015-06-04")
        ));
        assert!(matches!(
            preview_kind("/loi/code-civil/1728", None),
            Some(PreviewKind::Article { date: None, .. })
        ));
        assert!(matches!(
            preview_kind("/decision/z-hk-uk5YqPg", None),
            Some(PreviewKind::Decision { id }) if id == "z-hk-uk5YqPg"
        ));
    }

    #[test]
    fn preview_kind_rejects_other_shapes() {
        // Mention nue d'un texte, version datée, id vide, autres routes.
        assert!(preview_kind("/loi/code-civil", None).is_none());
        assert!(preview_kind("/loi/code-civil/1728/2016-10-01", None).is_none());
        assert!(preview_kind("/decision/", None).is_none());
        assert!(preview_kind("/decision/a/b", None).is_none());
        assert!(preview_kind("/recherche", None).is_none());
    }
}
