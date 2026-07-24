//! Rendu de snippets `<mark>…</mark>` (port de `HighlightedSnippet`).
//!
//! Le backend échappe tout SAUF les balises `<mark>`. On reproduit le `split`
//! sur `/(<mark>.*?<\/mark>)/g` du React : on émet des segments `Mark`/`Plain`
//! rendus en `<mark>`/texte brut. JAMAIS de `inner_html` (qui injecterait du
//! HTML non sanitisé) — Leptos échappe le texte des segments `Plain` et le
//! contenu des `Mark`.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{CitationSpan, CitationTarget};

use crate::components::hover_preview::{preview_kind, HoverPreview};

/// Segment d'un snippet : surligné ou texte brut.
enum Seg {
    Mark(String),
    Plain(String),
}

/// Découpe le texte en segments `<mark>` / hors-mark (non-greedy, comme la regex).
fn split(text: &str) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<mark>") {
        if start > 0 {
            segs.push(Seg::Plain(rest[..start].to_string()));
        }
        let after_open = &rest[start + "<mark>".len()..];
        match after_open.find("</mark>") {
            Some(end) => {
                segs.push(Seg::Mark(after_open[..end].to_string()));
                rest = &after_open[end + "</mark>".len()..];
            }
            None => {
                // `<mark>` sans fermeture : on traite le reste comme texte brut.
                segs.push(Seg::Plain(rest[start..].to_string()));
                rest = "";
                break;
            }
        }
    }
    if !rest.is_empty() {
        segs.push(Seg::Plain(rest.to_string()));
    }
    segs
}

/// Segment d'un paragraphe citable : texte brut ou mention de citation. Une
/// mention porte ≥1 `targets` : 1 → lien simple (ou souligné non-cliquable si non
/// résolu) ; ≥2 → menu déroulant (citation multi-articles partageant un span).
enum CiteSeg {
    Plain(String),
    Cite {
        text: String,
        targets: Vec<CitationTarget>,
    },
}

/// Découpe `text` (codepoints) aux bornes des `spans`, supposés disjoints et
/// triés par début (contrat de `spans_for_range` côté API), demi-ouverts
/// `[start, end)` en CODEPOINTS locaux. Émet une suite alternée texte/citation.
/// Spans vides ⇒ un seul segment `Plain`. Les spans hors bornes (offsets
/// incohérents) sont ignorés défensivement à la seule frontière de découpe.
fn split_spans(text: &str, spans: &[CitationSpan]) -> Vec<CiteSeg> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut segs = Vec::new();
    let mut cursor = 0usize;
    for span in spans {
        // Borne au texte ; ignore un span dégénéré ou hors plage.
        if span.start < cursor || span.end > n || span.start >= span.end {
            continue;
        }
        if span.start > cursor {
            segs.push(CiteSeg::Plain(chars[cursor..span.start].iter().collect()));
        }
        segs.push(CiteSeg::Cite {
            text: chars[span.start..span.end].iter().collect(),
            targets: span.targets.clone(),
        });
        cursor = span.end;
    }
    if cursor < n {
        segs.push(CiteSeg::Plain(chars[cursor..].iter().collect()));
    }
    segs
}

/// Rend une mention de citation : 1 cible → `<a>` (enveloppé d'une hover card
/// quand le href est prévisualisable, ADR 0168 — `at_date` = date de la
/// décision hôte, pour servir la version d'article qu'elle appliquait) ;
/// ≥2 cibles → [`CiteMenu`]. Une cible sans `href` n'est pas décorée : l'API ne
/// fabrique un span que résolu (mort du pointillé, 2026-07-05) — texte brut en
/// repli défensif de DTO.
fn cite_view(text: String, targets: Vec<CitationTarget>, at_date: Option<&str>) -> AnyView {
    let mut linked: Vec<(String, String)> = targets
        .into_iter()
        .filter_map(|t| Some((t.href?, t.label)))
        .collect();
    if linked.len() <= 1 {
        return match linked.pop() {
            Some((href, label)) => match preview_kind(&href, at_date) {
                // Carte hover : la tooltip native `title` ferait doublon et
                // flotterait par-dessus le panneau — on l'omet (la carte porte
                // déjà le label et bien plus).
                Some(kind) => {
                    let link = view! {
                        <A
                            href=href
                            attr:class="underline underline-offset-2 hover:text-[var(--color-accent)]"
                        >
                            {text}
                        </A>
                    };
                    view! { <HoverPreview kind=kind>{link}</HoverPreview> }.into_any()
                }
                // Sans carte, le `title` reste la seule glose du lien.
                None => view! {
                    <A
                        href=href
                        attr:title=label
                        attr:class="underline underline-offset-2 hover:text-[var(--color-accent)]"
                    >
                        {text}
                    </A>
                }
                .into_any(),
            },
            None => view! { <span>{text}</span> }.into_any(),
        };
    }
    view! { <CiteMenu text=text linked=linked /> }.into_any()
}

/// Menu déroulant d'une citation multi-cibles (plage d'articles, mention
/// partagée). PHRASING CONTENT uniquement (`span`/`button`/`a`) : un
/// `<details>` est du contenu de flux — le parseur HTML fermerait le `<p>`
/// parent et casserait la ligne. Même patron que `DropdownSelect` : signal +
/// panneau absolu, fermeture au clic extérieur et à Échap ; les `<a>` restent
/// dans le DOM au SSR (crawlables), le panneau n'est masqué que par classe.
#[component]
fn CiteMenu(text: String, linked: Vec<(String, String)>) -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Span>::new();

    // Ilot client : `window_event_listener` est inerte côté SSR (pas de window).
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        let click_handle = window_event_listener(leptos::ev::mousedown, move |ev| {
            if !open.get_untracked() {
                return;
            }
            let inside = container_ref
                .get_untracked()
                .zip(ev.target())
                .and_then(|(el, target)| {
                    target
                        .dyn_ref::<web_sys::Node>()
                        .map(|node| el.contains(Some(node)))
                })
                .unwrap_or(false);
            if !inside {
                open.set(false);
            }
        });
        on_cleanup(move || click_handle.remove());

        let esc_handle = window_event_listener(leptos::ev::keydown, move |ev| {
            if ev.key() == "Escape" && open.get_untracked() {
                open.set(false);
            }
        });
        on_cleanup(move || esc_handle.remove());
    }

    let items = linked
        .into_iter()
        .map(|(href, label)| {
            view! {
                <A
                    href=href
                    attr:class="block whitespace-nowrap px-3 py-1.5 text-left hover:bg-[var(--color-accent-soft)]"
                    on:click=move |_| open.set(false)
                >
                    {label}
                </A>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <span node_ref=container_ref class="relative">
            <button
                type="button"
                aria-haspopup="true"
                aria-expanded=move || open.get().then_some("true")
                on:click=move |_| open.update(|o| *o = !*o)
                class="cursor-pointer underline underline-offset-2 hover:text-[var(--color-accent)]"
            >
                {text}
            </button>
            <span
                class="absolute left-0 top-full z-10 mt-1 min-w-max rounded border border-[var(--color-rule)] bg-[var(--color-background)] py-1 text-sm shadow-lg"
                class:hidden=move || !open.get()
            >
                {items}
            </span>
        </span>
    }
}

/// Rend un paragraphe de décision avec ses citations cliquables (ADR 0134).
/// Overlay composé SERVER-SIDE via [`split_spans`] (cf. [`cite_view`]), le reste
/// en texte brut. JAMAIS d'`inner_html` — Leptos échappe chaque segment, SSR
/// crawlable. Aucun span ⇒ texte brut tel quel.
#[component]
pub fn CitedParagraph(
    text: String,
    spans: Vec<CitationSpan>,
    /// Date de la décision hôte : les hover cards d'articles servent la
    /// version en vigueur à cette date (ADR 0168).
    #[prop(optional_no_strip)]
    at_date: Option<String>,
) -> impl IntoView {
    if spans.is_empty() {
        return view! { <p>{text}</p> }.into_any();
    }
    let views = split_spans(&text, &spans)
        .into_iter()
        .map(|seg| match seg {
            CiteSeg::Plain(s) => view! { <span>{s}</span> }.into_any(),
            CiteSeg::Cite { text, targets } => cite_view(text, targets, at_date.as_deref()),
        })
        .collect::<Vec<_>>();
    view! { <p>{views}</p> }.into_any()
}

/// Rend le corps d'un article de norme avec ses renvois cliquables
/// (ADR 0217) : même composition que [`CitedParagraph`] (offsets codepoints
/// demi-ouverts sur le texte entier, jamais d'`inner_html`), en un seul bloc
/// `<div>` — la mise en forme du corps repose sur `whitespace-pre-line`, pas
/// sur des paragraphes. La date d'un renvoi Chronolégi voyage dans le href
/// lui-même (l'API date les liens quand la lecture l'est) : pas d'`at_date`
/// de contexte.
#[component]
pub fn CitedBlock(text: String, spans: Vec<CitationSpan>, class: &'static str) -> impl IntoView {
    if spans.is_empty() {
        return view! { <div class=class>{text}</div> }.into_any();
    }
    let views = split_spans(&text, &spans)
        .into_iter()
        .map(|seg| match seg {
            CiteSeg::Plain(s) => view! { <span>{s}</span> }.into_any(),
            CiteSeg::Cite { text, targets } => cite_view(text, targets, None),
        })
        .collect::<Vec<_>>();
    view! { <div class=class>{views}</div> }.into_any()
}

/// Rend un texte avec ses `<mark>` surlignés (mêmes classes que le TSX).
#[component]
pub fn Highlighted(#[prop(into)] text: String) -> impl IntoView {
    let views = split(&text)
        .into_iter()
        .map(|seg| match seg {
            Seg::Mark(s) => view! {
                <mark class="bg-[var(--color-accent-soft)] px-0.5 text-[var(--color-ink)]">
                    {s}
                </mark>
            }
            .into_any(),
            Seg::Plain(s) => view! { <span>{s}</span> }.into_any(),
        })
        .collect::<Vec<_>>();
    view! { {views} }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize, href: Option<&str>) -> CitationSpan {
        CitationSpan {
            start,
            end,
            targets: vec![CitationTarget {
                href: href.map(str::to_string),
                label: "Code civil 1240".to_string(),
            }],
        }
    }

    /// (c) Le split produit les bons segments texte/citation/texte, avec
    /// découpe en CODEPOINTS (texte multi-octets).
    #[test]
    fn split_spans_segments_text_cite_text() {
        // "voir article 1240 du code" — « 1240 » aux codepoints 13..17.
        let text = "voir article 1240 du code";
        let segs = split_spans(text, &[span(13, 17, Some("/texte/code-civil/1240"))]);
        assert_eq!(segs.len(), 3);
        match &segs[0] {
            CiteSeg::Plain(s) => assert_eq!(s, "voir article "),
            _ => panic!("segment 0 doit être Plain"),
        }
        match &segs[1] {
            CiteSeg::Cite { text, targets } => {
                assert_eq!(text, "1240");
                assert_eq!(targets[0].href.as_deref(), Some("/texte/code-civil/1240"));
            }
            _ => panic!("segment 1 doit être Cite"),
        }
        match &segs[2] {
            CiteSeg::Plain(s) => assert_eq!(s, " du code"),
            _ => panic!("segment 2 doit être Plain"),
        }
    }

    #[test]
    fn split_spans_multibyte_codepoint_offsets() {
        // « é » est 1 codepoint / 2 octets : la découpe doit indexer en codepoints.
        let text = "café 1240 thé";
        // « 1240 » = codepoints 5..9.
        let segs = split_spans(text, &[span(5, 9, None)]);
        assert_eq!(segs.len(), 3);
        match &segs[1] {
            CiteSeg::Cite { text, targets } => {
                assert_eq!(text, "1240");
                assert!(targets[0].href.is_none()); // non résolu → souligné non-cliquable
            }
            _ => panic!("segment 1 doit être Cite"),
        }
        match (&segs[0], &segs[2]) {
            (CiteSeg::Plain(a), CiteSeg::Plain(b)) => {
                assert_eq!(a, "café ");
                assert_eq!(b, " thé");
            }
            _ => panic!("segments 0/2 doivent être Plain"),
        }
    }

    #[test]
    fn split_spans_no_spans_is_single_plain() {
        let segs = split_spans("texte simple", &[]);
        assert_eq!(segs.len(), 1);
        matches!(&segs[0], CiteSeg::Plain(s) if s == "texte simple");
    }

    #[test]
    fn split_spans_carries_multiple_targets_for_one_span() {
        // « articles 1382 et 1383 du code civil » : un span, deux cibles → un seul
        // segment Cite portant les deux (rendu = menu déroulant côté composant).
        let text = "articles 1382 et 1383 du code civil";
        let multi = CitationSpan {
            start: 0,
            end: text.chars().count(),
            targets: vec![
                CitationTarget {
                    href: Some("/texte/code-civil/1382".to_string()),
                    label: "1382".to_string(),
                },
                CitationTarget {
                    href: Some("/texte/code-civil/1383".to_string()),
                    label: "1383".to_string(),
                },
            ],
        };
        let segs = split_spans(text, &[multi]);
        assert_eq!(segs.len(), 1);
        match &segs[0] {
            CiteSeg::Cite { targets, .. } => assert_eq!(targets.len(), 2),
            _ => panic!("doit être un seul segment Cite multi-cibles"),
        }
    }

    #[test]
    fn split_spans_multi_occurrence_emits_each() {
        // Deux mentions disjointes → deux segments Cite.
        let text = "1240 puis 1241";
        let segs = split_spans(text, &[span(0, 4, Some("/a")), span(10, 14, Some("/b"))]);
        // Cite("1240"), Plain(" puis "), Cite("1241")
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[0], CiteSeg::Cite { text, .. } if text == "1240"));
        assert!(matches!(&segs[2], CiteSeg::Cite { text, .. } if text == "1241"));
    }
}
