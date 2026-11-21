//! Rendu de snippets `<mark>…</mark>`.
//!
//! Le backend échappe tout SAUF les balises `<mark>` : on émet des segments
//! `Mark`/`Plain` rendus en `<mark>`/texte brut. JAMAIS de `inner_html` — Leptos
//! échappe le texte des segments `Plain` et le contenu des `Mark`.

use std::ops::Range;

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{CitationSpan, CitationTarget};

use crate::components::hover_preview::{preview_kind, HoverPreview};

/// Segment d'un snippet : surligné ou texte brut.
enum Seg {
    Mark(String),
    Plain(String),
}

/// Découpe le texte en segments `<mark>` / hors-mark (non-greedy).
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
/// Spans vides ⇒ un seul segment `Plain`. Les spans hors bornes sont ignorés.
fn split_spans(text: &str, spans: &[CitationSpan]) -> Vec<CiteSeg> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut segs = Vec::new();
    let mut cursor = 0usize;
    for span in spans {
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
/// quand le href est prévisualisable — `at_date` = date de la décision hôte, pour
/// servir la version d'article qu'elle appliquait) ; ≥2 cibles → [`CiteMenu`].
/// Une cible sans `href` n'est pas décorée : texte brut.
fn cite_view(text: Vec<Run>, targets: Vec<CitationTarget>, at_date: Option<&str>) -> AnyView {
    let mut linked: Vec<(String, String)> = targets
        .into_iter()
        .filter_map(|t| Some((t.href?, t.label)))
        .collect();
    if linked.len() <= 1 {
        return match linked.pop() {
            Some((href, label)) => match preview_kind(&href, at_date) {
                Some(kind) => {
                    let link = view! {
                        <A
                            href=href
                            attr:class="underline underline-offset-2 hover:text-[var(--color-accent)]"
                        >
                            {runs_view(text)}
                        </A>
                    };
                    view! { <HoverPreview kind=kind>{link}</HoverPreview> }.into_any()
                }
                None => view! {
                    <A
                        href=href
                        attr:title=label
                        attr:class="underline underline-offset-2 hover:text-[var(--color-accent)]"
                    >
                        {runs_view(text)}
                    </A>
                }
                .into_any(),
            },
            None => view! { <span>{runs_view(text)}</span> }.into_any(),
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
fn CiteMenu(text: Vec<Run>, linked: Vec<(String, String)>) -> impl IntoView {
    let open = RwSignal::new(false);
    let container_ref = NodeRef::<leptos::html::Span>::new();

    #[cfg(feature = "hydrate")]
    {
        use crate::components::ui::panel::{dismiss, Dismiss};
        dismiss(
            container_ref,
            move || open.get_untracked(),
            move || open.set(false),
            Dismiss {
                escape: true,
                scroll: false,
            },
        );
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
                {runs_view(text)}
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

/// Chrome's HTML parser splits a long text node, and hydration then meets two nodes
/// where the server wrote one. A char is at most two UTF-16 units: half the parser's cap.
const TEXT_NODE_MAX_CHARS: usize = 32_768;

fn text_node_chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    for (i, (at, _)) in text.char_indices().enumerate() {
        if i > 0 && i % TEXT_NODE_MAX_CHARS == 0 {
            chunks.push(&text[start..at]);
            start = at;
        }
    }
    chunks.push(&text[start..]);
    chunks
}

pub(crate) fn plain_text(text: String) -> AnyView {
    if text.len() <= TEXT_NODE_MAX_CHARS {
        return text.into_any();
    }
    text_node_chunks(&text)
        .into_iter()
        .map(|c| view! { <span>{c.to_string()}</span> })
        .collect_view()
        .into_any()
}

/// Rend un paragraphe de décision avec ses citations cliquables. Overlay composé
/// SERVER-SIDE via [`split_spans`] (cf. [`cite_view`]), le reste en texte brut.
/// JAMAIS d'`inner_html` — Leptos échappe chaque segment, SSR crawlable. Aucun
/// span ⇒ texte brut tel quel.
#[component]
pub fn CitedParagraph(
    text: String,
    spans: Vec<CitationSpan>,
    /// Date de la décision hôte : les hover cards d'articles servent la version en
    /// vigueur à cette date.
    #[prop(optional_no_strip)]
    at_date: Option<String>,
    /// What the search matched, in codepoints.
    #[prop(optional)]
    hits: Vec<Range<usize>>,
) -> impl IntoView {
    if spans.is_empty() && hits.is_empty() {
        return view! { <p>{plain_text(text)}</p> }.into_any();
    }
    let mut at = 0;
    let views = split_spans(&text, &spans)
        .into_iter()
        .map(|seg| match seg {
            CiteSeg::Plain(s) => {
                let runs = runs(&s, &mut at, &hits);
                view! { <span>{runs_view(runs)}</span> }.into_any()
            }
            CiteSeg::Cite { text, targets } => {
                cite_view(runs(&text, &mut at, &hits), targets, at_date.as_deref())
            }
        })
        .collect::<Vec<_>>();
    view! { <p>{views}</p> }.into_any()
}

/// A piece of a paragraph, highlighted when the search matched it.
struct Run {
    text: String,
    hit: bool,
}

/// `text`, starting at codepoint `*at` of its paragraph, cut at the `hits`;
/// moves `*at` past it.
fn runs(text: &str, at: &mut usize, hits: &[Range<usize>]) -> Vec<Run> {
    let chars: Vec<char> = text.chars().collect();
    let (start, end) = (*at, *at + chars.len());
    *at = end;
    let piece = |a: usize, b: usize| chars[a - start..b - start].iter().collect::<String>();
    let mut out = Vec::new();
    let mut cursor = start;
    for r in hits.iter().filter(|r| r.start < end && r.end > start) {
        let (a, b) = (r.start.max(start), r.end.min(end));
        if a > cursor {
            out.push(Run {
                text: piece(cursor, a),
                hit: false,
            });
        }
        out.push(Run {
            text: piece(a, b),
            hit: true,
        });
        cursor = b;
    }
    if cursor < end {
        out.push(Run {
            text: piece(cursor, end),
            hit: false,
        });
    }
    out
}

fn runs_view(mut runs: Vec<Run>) -> AnyView {
    if let [Run { hit: false, .. }] = runs.as_slice() {
        return plain_text(runs.pop().expect("one run").text);
    }
    runs.into_iter()
        .map(|run| {
            if run.hit {
                view! { <mark data-hit="">{run.text}</mark> }.into_any()
            } else {
                plain_text(run.text)
            }
        })
        .collect_view()
        .into_any()
}

/// Rend le corps d'un article de norme avec ses renvois cliquables : même
/// composition que [`CitedParagraph`] (offsets codepoints demi-ouverts sur le
/// texte entier, jamais d'`inner_html`), en un seul bloc `<div>` — la mise en
/// forme du corps repose sur `whitespace-pre-line`, pas sur des paragraphes. La
/// date d'un renvoi Chronolégi voyage dans le href lui-même : pas d'`at_date` de
/// contexte.
#[component]
pub fn CitedBlock(text: String, spans: Vec<CitationSpan>, class: &'static str) -> impl IntoView {
    if spans.is_empty() {
        return view! { <div class=class>{plain_text(text)}</div> }.into_any();
    }
    let views = split_spans(&text, &spans)
        .into_iter()
        .map(|seg| match seg {
            CiteSeg::Plain(s) => view! { <span>{plain_text(s)}</span> }.into_any(),
            CiteSeg::Cite { text, targets } => {
                cite_view(vec![Run { text, hit: false }], targets, None)
            }
        })
        .collect::<Vec<_>>();
    view! { <div class=class>{views}</div> }.into_any()
}

/// Rend un texte avec ses `<mark>` surlignés.
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

    #[test]
    fn text_node_chunks_stay_under_the_parser_split() {
        let text = "é𝄞a".repeat(30_000);
        let chunks = text_node_chunks(&text);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.encode_utf16().count() <= 65_536));
    }

    #[test]
    fn split_spans_segments_text_cite_text() {
        let text = "voir article 1240 du code";
        let segs = split_spans(text, &[span(13, 17, Some("/norm/fr/code-civil/1240"))]);
        assert_eq!(segs.len(), 3);
        match &segs[0] {
            CiteSeg::Plain(s) => assert_eq!(s, "voir article "),
            _ => panic!("segment 0 doit être Plain"),
        }
        match &segs[1] {
            CiteSeg::Cite { text, targets } => {
                assert_eq!(text, "1240");
                assert_eq!(targets[0].href.as_deref(), Some("/norm/fr/code-civil/1240"));
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
        let text = "café 1240 thé";
        let segs = split_spans(text, &[span(5, 9, None)]);
        assert_eq!(segs.len(), 3);
        match &segs[1] {
            CiteSeg::Cite { text, targets } => {
                assert_eq!(text, "1240");
                assert!(targets[0].href.is_none());
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
        let text = "articles 1382 et 1383 du code civil";
        let multi = CitationSpan {
            start: 0,
            end: text.chars().count(),
            targets: vec![
                CitationTarget {
                    href: Some("/norm/fr/code-civil/1382".to_string()),
                    label: "1382".to_string(),
                },
                CitationTarget {
                    href: Some("/norm/fr/code-civil/1383".to_string()),
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
        let text = "1240 puis 1241";
        let segs = split_spans(text, &[span(0, 4, Some("/a")), span(10, 14, Some("/b"))]);
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[0], CiteSeg::Cite { text, .. } if text == "1240"));
        assert!(matches!(&segs[2], CiteSeg::Cite { text, .. } if text == "1241"));
    }
}
