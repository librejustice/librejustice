//! The document itself: drawn pages with their overlay, or the text feed. Both
//! read the same hits, so a name found is a name shown either way. An image is
//! a raster on a canvas; a PDF or a laid-out page is the SVG the module drew,
//! vector, so a zoom only resizes its box.

use leptos::prelude::*;

use super::state::{Hit, Mode, PageWord, Session, TextBlock, View, ERASED};

#[cfg(feature = "hydrate")]
use super::engine;

/// The glyph quads of one hit merged into one box per line, `[x0, y0, x1, y1]`.
fn line_boxes(quads: &[[[f64; 2]; 4]]) -> Vec<[f64; 4]> {
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    for quad in quads {
        let (xs, ys) = (quad.map(|p| p[0]), quad.map(|p| p[1]));
        let glyph = [
            xs.into_iter().fold(f64::MAX, f64::min),
            ys.into_iter().fold(f64::MAX, f64::min),
            xs.into_iter().fold(f64::MIN, f64::max),
            ys.into_iter().fold(f64::MIN, f64::max),
        ];
        match boxes.last_mut() {
            Some(line) if glyph[1] < line[3] && glyph[3] > line[1] => {
                line[0] = line[0].min(glyph[0]);
                line[1] = line[1].min(glyph[1]);
                line[2] = line[2].max(glyph[2]);
                line[3] = line[3].max(glyph[3]);
            }
            _ => boxes.push(glyph),
        }
    }
    boxes
}

/// The hits no other one covers: an occurrence inside a longer one, `Dupont`
/// inside `Jean Dupont`, takes no label of its own.
fn outermost(hits: &[Hit]) -> Vec<&Hit> {
    hits.iter()
        .enumerate()
        .filter(|(at, hit)| {
            !hits.iter().enumerate().any(|(other, wider)| {
                other != *at
                    && wider.start <= hit.start
                    && hit.end <= wider.end
                    && (wider.end - wider.start > hit.end - hit.start || other < *at)
            })
        })
        .map(|(_, hit)| hit)
        .collect()
}

#[component]
pub fn Viewer(session: Session) -> impl IntoView {
    let viewport = NodeRef::<leptos::html::Div>::new();

    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(node) = viewport.get() {
                let element: web_sys::Element = node.into();
                engine::watch_pages(session, &element);
            }
        });
        Effect::new(move |_| {
            let _ = session.pages.get();
            let _ = session.generation.get();
            if session.fit.get() {
                engine::fit(session);
            }
        });
    }

    let pages = move || (0..session.pages.with(Vec::len)).collect::<Vec<_>>();
    let on_wheel = move |event: leptos::ev::WheelEvent| {
        if !(event.ctrl_key() || event.meta_key()) {
            return;
        }
        event.prevent_default();
        // A trackpad pinch arrives as ctrl+wheel steps of a few pixels, a mouse
        // notch as far larger ones: one gain for both makes the pinch crawl.
        #[cfg(feature = "hydrate")]
        {
            let delta = match event.delta_mode() {
                leptos::ev::WheelEvent::DOM_DELTA_LINE => event.delta_y() * 16.0,
                _ => event.delta_y(),
            };
            let gain = if delta.abs() < 50.0 { 0.01 } else { 0.002 };
            engine::zoom_at(session, (-delta * gain).exp(), &event);
        }
    };

    view! {
        <div
            node_ref=viewport
            id="redact-viewport"
            class="relative min-h-0 flex-1 overflow-auto bg-[var(--color-vellum)]"
            on:wheel=on_wheel
        >
            <Show
                when=move || session.view.get() == View::Pages
                fallback=move || view! { <TextFeed session=session /> }
            >
                <div id="redact-pages" class="flex w-max min-w-full flex-col items-center gap-6 p-6">
                    <For each=pages key=|index| *index let:index>
                        <Page session=session index=index />
                    </For>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn Page(session: Session, index: usize) -> impl IntoView {
    let host = NodeRef::<leptos::html::Div>::new();
    let canvas = NodeRef::<leptos::html::Canvas>::new();
    let sheet = NodeRef::<leptos::html::Div>::new();
    let laid_out = Memo::new(move |_| session.format.get().laid_out());
    let svg = Memo::new(move |_| session.format.get().drawn_as_svg());

    let size = move || {
        session
            .pages
            .with(|pages| pages.get(index).copied().unwrap_or((595.0, 842.0)))
    };
    let box_style = move || {
        let ((width, height), zoom) = (size(), session.zoom.get());
        format!("width:{}px;height:{}px", width * zoom, height * zoom)
    };
    let view_box = move || {
        let (width, height) = size();
        format!("0 0 {width} {height}")
    };
    let flip = move || match session.format.get().flips_y() {
        true => format!("translate(0 {}) scale(1 -1)", size().1),
        false => String::new(),
    };

    let words: RwSignal<Vec<Vec<PageWord>>> = RwSignal::new(Vec::new());

    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(node) = host.get() {
                let element: web_sys::Element = node.into();
                engine::observe_page(&element);
            }
        });
        let read: RwSignal<Option<(usize, usize)>> = RwSignal::new(None);
        Effect::new(move |_| {
            let seen = session.visible.with(|pages| pages.contains(&index));
            let stamp = (session.generation.get(), session.characters.get());
            if laid_out.get() {
                if words.with_untracked(|lines| !lines.is_empty()) {
                    words.set(Vec::new());
                }
                return;
            }
            if seen && read.get_untracked() != Some(stamp) {
                words.set(engine::page_text(index));
                read.set(Some(stamp));
            }
        });
        let painted: RwSignal<Option<(u64, usize)>> = RwSignal::new(None);
        let asked = StoredValue::new(0_u64);
        Effect::new(move |_| {
            let seen = session.visible.with(|pages| pages.contains(&index));
            let zoom = session.zoom.get();
            let generation = session.generation.get();
            if !seen || canvas.get().is_none() || svg.get() {
                return;
            }
            let stamp = ((zoom * 1000.0) as u64, generation);
            let last = painted.get_untracked();
            if last == Some(stamp) {
                return;
            }
            let paint = move || {
                if let Some(node) = canvas.get_untracked() {
                    if engine::paint(&node, zoom).is_ok() {
                        painted.set(Some(stamp));
                    }
                }
            };
            // A zoom gesture fires dozens of steps: only the one it settles on is rasterised.
            match last {
                Some((_, painted_generation)) if painted_generation == generation => {
                    asked.update_value(|n| *n += 1);
                    let ticket = asked.get_value();
                    set_timeout(
                        move || {
                            if asked.get_value() == ticket {
                                paint();
                            }
                        },
                        std::time::Duration::from_millis(160),
                    );
                }
                _ => paint(),
            }
        });
        let inserted: RwSignal<Option<usize>> = RwSignal::new(None);
        Effect::new(move |_| {
            let seen = session.visible.with(|pages| pages.contains(&index));
            let generation = session.generation.get();
            let Some(node) = sheet.get() else { return };
            if !seen || !svg.get() || inserted.get_untracked() == Some(generation) {
                return;
            }
            if let Ok(svg) = engine::page_svg(index, session.format.get_untracked()) {
                node.set_inner_html(&svg);
                inserted.set(Some(generation));
            }
        });
    }

    let marks = move || {
        session
            .live_hits()
            .into_iter()
            .filter(|hit| hit.page == Some(index))
            .flat_map(|hit| line_boxes(&hit.quads))
            .map(|[x0, y0, x1, y1]| {
                view! {
                    <rect
                        x=x0
                        y=y0
                        width=x1 - x0
                        height=y1 - y0
                        rx="1"
                        fill="rgba(140, 20, 40, 0.32)"
                    />
                }
            })
            .collect_view()
    };
    let focused = move || {
        let hit = session.focused().filter(|hit| hit.page == Some(index))?;
        let pad = 1.5;
        Some(
            line_boxes(&hit.quads)
                .into_iter()
                .map(|[x0, y0, x1, y1]| {
                    view! {
                        <rect
                            x=x0 - pad
                            y=y0 - pad
                            width=x1 - x0 + 2.0 * pad
                            height=y1 - y0 + 2.0 * pad
                            rx="2"
                            fill="none"
                            stroke="var(--color-accent)"
                            stroke-width="2"
                            vector-effect="non-scaling-stroke"
                        />
                    }
                })
                .collect_view(),
        )
    };
    // Each label covers the first line of its hit, sized on that line but
    // never under 11 px, so it stays readable however far the page is zoomed out.
    let tags = move || {
        if !laid_out.get() {
            return Vec::new();
        }
        let ((width, height), zoom) = (size(), session.zoom.get());
        let texts = session.hits.with(|found| found.needles.clone());
        let hits: Vec<Hit> = session
            .live_hits()
            .into_iter()
            .filter(|hit| hit.page == Some(index))
            .collect();
        outermost(&hits)
            .into_iter()
            .filter_map(|hit| {
                let label = session.label_of(texts.get(hit.needle)?)?;
                let [left, top, right, bottom] = *line_boxes(&hit.quads).first()?;
                let line = (bottom - top) * zoom;
                let pixels = (line * 0.75).max(11.0);
                let style = format!(
                    "left:{}%;top:{}%;min-width:{}px;font-size:{pixels}px;line-height:{}px",
                    left / width * 100.0,
                    (top + bottom) / 2.0 / height * 100.0,
                    (right - left) * zoom,
                    line.max(pixels * 1.25)
                );
                let (face, words) = match label.is_empty() {
                    true => ("italic", ERASED.to_string()),
                    false => ("", label),
                };
                Some(view! {
                    <span
                        class=format!("pointer-events-none absolute z-10 -translate-y-1/2 whitespace-nowrap rounded-[2px] bg-[var(--color-accent)] px-0.5 text-[var(--color-accent-foreground)] ring-1 ring-white {face}")
                        style=style
                    >
                        {words}
                    </span>
                })
            })
            .collect::<Vec<_>>()
    };
    let boxes = move || {
        let selected = session.selected.get();
        let drawn = session
            .drawing
            .get()
            .filter(|rect| rect.page == index)
            .map(|rect| (None, rect));
        session
            .rects
            .get()
            .into_iter()
            .enumerate()
            .filter(|(_, rect)| rect.page == index)
            .map(|(at, rect)| (Some(at), rect))
            .chain(drawn)
            .map(|(at, rect)| {
                let stroke = match at.is_some() && at == selected {
                    true => "var(--color-accent)",
                    false => "none",
                };
                view! {
                    <rect
                        x=rect.x
                        y=rect.y
                        width=rect.width
                        height=rect.height
                        fill="rgba(0, 0, 0, 0.82)"
                        stroke=stroke
                        stroke-width="2"
                        vector-effect="non-scaling-stroke"
                    />
                }
            })
            .collect_view()
    };
    let cross = move || {
        let at = session.selected.get()?;
        let rect = session.rects.with(|rects| rects.get(at).copied())?;
        if rect.page != index {
            return None;
        }
        let (width, height) = size();
        let top = match session.format.get().flips_y() {
            true => height - rect.y - rect.height,
            false => rect.y,
        };
        let style = format!(
            "left:{}%;top:{}%",
            (rect.x + rect.width) / width * 100.0,
            top / height * 100.0
        );
        Some(view! {
            <button
                type="button"
                aria-label="Retirer ce rectangle"
                title="Retirer (Suppr)"
                class="absolute z-10 flex h-6 w-6 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full bg-white text-sm leading-none text-[var(--color-ink)] shadow ring-1 ring-[var(--color-rule)] hover:text-[var(--color-accent)]"
                style=style
                on:pointerdown=|event| event.stop_propagation()
                on:click=move |_| session.remove_rect(at)
            >
                "×"
            </button>
        })
    };

    let text_layer = move || {
        let page_height = size().1;
        let flip = session.format.get().flips_y();
        words.with(|lines| {
            lines
                .iter()
                .map(|line| {
                    let line_size = line
                        .iter()
                        .map(|word| word.bounds[3] - word.bounds[1])
                        .fold(1.0_f64, f64::max);
                    let spans = line
                        .iter()
                        .map(|word| {
                            let [x0, y0, x1, y1] = word.bounds;
                            let (top, bottom) = match flip {
                                true => (page_height - y1, page_height - y0),
                                false => (y0, y1),
                            };
                            let size = (bottom - top).max(1.0);
                            view! {
                                <tspan
                                    x=x0
                                    y=bottom - 0.2 * size
                                    font-size=size
                                    textLength=(x1 - x0).max(1.0)
                                    lengthAdjust="spacingAndGlyphs"
                                >
                                    {word.text.clone()}
                                </tspan>
                                " "
                            }
                        })
                        .collect_view();
                    view! { <text font-size=line_size>{spans}</text> }
                })
                .collect_view()
        })
    };
    // A laid-out page carries its own text, selectable where it is drawn.
    let text_class = move || {
        match (session.mode.get(), laid_out.get()) {
        (Mode::Read, false) => {
            "absolute inset-0 h-full w-full cursor-text select-text selection:bg-[color-mix(in_oklch,var(--color-ink)_22%,transparent)] selection:fill-transparent"
        }
        _ => "pointer-events-none absolute inset-0 h-full w-full select-none",
    }
    };

    let on_down = move |event: leptos::ev::PointerEvent| {
        let _ = &event;
        #[cfg(feature = "hydrate")]
        engine::draw(session, index, engine::Phase::Down, &event);
    };
    let on_move = move |event: leptos::ev::PointerEvent| {
        let _ = &event;
        #[cfg(feature = "hydrate")]
        engine::draw(session, index, engine::Phase::Move, &event);
    };
    let on_up = move |event: leptos::ev::PointerEvent| {
        let _ = &event;
        #[cfg(feature = "hydrate")]
        engine::draw(session, index, engine::Phase::Up, &event);
    };

    view! {
        <div
            node_ref=host
            id=format!("redact-page-{index}")
            data-page=index.to_string()
            class=move || match session.mode.get() {
                Mode::Draw => {
                    "relative touch-none cursor-crosshair shadow-md ring-1 ring-[var(--color-rule)]"
                }
                Mode::Read => "relative shadow-md ring-1 ring-[var(--color-rule)]",
            }
            style=box_style
            on:pointerdown=on_down
            on:pointermove=on_move
            on:pointerup=on_up
            on:pointercancel=on_up
        >
            {move || match svg.get() {
                true => {
                    view! {
                        <div
                            node_ref=sheet
                            class="h-full w-full bg-white [&>svg]:block [&>svg]:h-full [&>svg]:w-full"
                        />
                    }
                        .into_any()
                }
                false => {
                    view! { <canvas node_ref=canvas class="block h-full w-full bg-white" /> }
                        .into_any()
                }
            }}
            <svg
                class="pointer-events-none absolute inset-0 h-full w-full"
                viewBox=view_box
                preserveAspectRatio="none"
                aria-hidden="true"
            >
                <g transform=flip>{marks} {focused} {boxes}</g>
            </svg>
            {tags}
            {cross}
            <svg
                class=text_class
                viewBox=view_box
                preserveAspectRatio="none"
                fill="transparent"
                font-family="sans-serif"
            >
                {text_layer}
            </svg>
        </div>
    }
}

/// How a run of the text view is marked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mark {
    Plain,
    Hit,
    Focused,
}

/// Cut `block` into runs, each marked by the hits covering it, `focused` over
/// the others.
fn runs(block: &TextBlock, hits: &[Hit], focused: Option<&Hit>) -> Vec<(usize, String, Mark)> {
    let characters: Vec<char> = block.text.chars().collect();
    let mut covered = vec![Mark::Plain; characters.len()];
    let marked = hits.iter().map(|hit| (hit, Mark::Hit));
    for (hit, mark) in marked.chain(focused.map(|hit| (hit, Mark::Focused))) {
        if hit.end <= block.start || hit.start >= block.start + characters.len() {
            continue;
        }
        let from = hit.start.saturating_sub(block.start);
        let to = (hit.end - block.start).min(characters.len());
        for flag in covered.iter_mut().take(to).skip(from) {
            *flag = mark;
        }
    }
    let mut out: Vec<(usize, String, Mark)> = Vec::new();
    for (character, flag) in characters.into_iter().zip(covered) {
        match out.last_mut() {
            Some((_, text, last)) if *last == flag => text.push(character),
            _ => out.push((out.len(), character.to_string(), flag)),
        }
    }
    out
}

#[component]
fn TextFeed(session: Session) -> impl IntoView {
    let starts = move || {
        session
            .blocks
            .with(|blocks| blocks.iter().map(|b| b.start).collect::<Vec<_>>())
    };
    let style = move || format!("font-size:{}rem", 0.95 * session.zoom.get().clamp(0.6, 2.0));

    view! {
        <div class="mx-auto flex max-w-3xl flex-col gap-3 p-6" style=style>
            <For each=starts key=|start| *start let:start>
                <Paragraph session=session start=start />
            </For>
        </div>
    }
}

#[component]
fn Paragraph(session: Session, start: usize) -> impl IntoView {
    let parts = Memo::new(move |_| {
        let hits = session.live_hits();
        let focused = session.focused();
        session.blocks.with(|blocks| {
            blocks
                .iter()
                .find(|block| block.start == start)
                .map(|block| runs(block, &hits, focused.as_ref()))
                .unwrap_or_default()
        })
    });

    view! {
        <p id=format!("redact-block-{start}") class="leading-relaxed text-[var(--color-ink)]">
            {move || {
                parts
                    .get()
                    .into_iter()
                    .map(|(_, text, marked)| view! { <Run text=text marked=marked /> })
                    .collect_view()
            }}
        </p>
    }
}

#[component]
fn Run(text: String, marked: Mark) -> impl IntoView {
    let class = match marked {
        Mark::Hit => "rounded-sm bg-[var(--color-accent-soft)] text-[var(--color-accent)]",
        Mark::Focused => {
            "rounded-sm bg-[var(--color-accent-soft)] text-[var(--color-accent)] ring-2 ring-[var(--color-accent)]"
        }
        Mark::Plain => "",
    };
    view! { <span class=class>{text}</span> }
}
