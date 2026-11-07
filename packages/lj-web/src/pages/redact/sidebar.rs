//! What will be removed: the list, where each entry came from, and the verdict
//! once the pass has run.

use leptos::either::Either;
use leptos::prelude::*;

use super::outlet::{Handover, Outlets};
use super::state::{Check, Eye, Reading, Session, Verdict, ERASED, FAMILIES};

#[cfg(feature = "hydrate")]
use super::engine;

const SECTION: &str =
    "text-xs font-medium uppercase tracking-widest text-[var(--color-ink-subtle)]";
const OUTLET: &str = "h-11 min-w-0 flex-1 truncate rounded-md px-4 text-sm font-medium md:w-full md:flex-none disabled:pointer-events-none disabled:opacity-40";
const FOREMOST: &str =
    "bg-[var(--color-accent)] text-[var(--color-accent-foreground)] hover:opacity-90";
const ASIDE: &str =
    "border border-[var(--color-rule)] text-[var(--color-ink)] hover:bg-[var(--color-vellum)]";

#[component]
fn Cross() -> impl IntoView {
    view! {
        <svg
            viewBox="0 0 16 16"
            class="h-3.5 w-3.5"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            aria-hidden="true"
        >
            <path d="M4 4 L12 12 M12 4 L4 12" />
        </svg>
    }
}

#[component]
fn NeedleRow(session: Session, text: String) -> impl IntoView {
    let key = text.clone();
    let checked = {
        let key = key.clone();
        move || {
            session
                .needles
                .with(|list| list.iter().any(|n| n.text == key && n.on))
        }
    };
    let count = {
        let key = key.clone();
        move || {
            let total = session.count_of(&key);
            match session.focus.get() {
                Some((text, rank)) if text == key && rank < total => {
                    format!("{}/{total}", rank + 1)
                }
                _ => total.to_string(),
            }
        }
    };
    let next = {
        let key = key.clone();
        move |_| {
            let hit = session.step(&key);
            #[cfg(feature = "hydrate")]
            if let Some(hit) = hit {
                engine::show(session, &hit);
            }
            #[cfg(not(feature = "hydrate"))]
            let _ = hit;
        }
    };
    let toggle = {
        let key = key.clone();
        move |_| {
            session.needles.update(|list| {
                if let Some(needle) = list.iter_mut().find(|n| n.text == key) {
                    needle.on = !needle.on;
                }
            })
        }
    };
    let labelled = {
        let key = key.clone();
        move || session.label_of(&key).is_some()
    };
    let remove = move |_| {
        session
            .needles
            .update(|list| list.retain(|n| n.text != key))
    };
    let title = text.clone();
    let tagged = text.clone();

    view! {
        <div class="flex items-center gap-2 rounded-sm px-1 py-1 hover:bg-[var(--color-vellum)]">
            <input
                type="checkbox"
                class="h-4 w-4 shrink-0 cursor-pointer accent-[var(--color-accent)]"
                aria-label="Inclure dans l'anonymisation"
                prop:checked=checked
                on:change=toggle
            />
            <button
                type="button"
                class="min-w-0 flex-1 truncate text-left text-sm text-[var(--color-ink)] hover:underline"
                title=format!("{title} : aller à l'occurrence suivante")
                on:click=next
            >
                {text}
            </button>
            <Show when=labelled>
                <Tag session=session text=tagged.clone() />
            </Show>
            <span class="shrink-0 text-xs tabular-nums text-[var(--color-ink-subtle)]">{count}</span>
            <button
                type="button"
                class="shrink-0 rounded-sm p-1 text-[var(--color-ink-subtle)] hover:text-[var(--color-accent)]"
                aria-label="Retirer de la liste"
                on:click=remove
            >
                <Cross />
            </button>
        </div>
    }
}

const TAG: &str = "max-w-32 shrink-0 truncate rounded-sm border bg-[var(--color-parchment)] px-1.5 font-mono text-xs leading-[18px] hover:border-[var(--color-ink-muted)]";

/// What replaces the string in a Word document, edited in place. An emptied
/// label erases the string.
#[component]
fn Tag(session: Session, text: String) -> impl IntoView {
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let field = NodeRef::<leptos::html::Input>::new();

    let shown = {
        let text = text.clone();
        Memo::new(move |_| session.label_of(&text).unwrap_or_default())
    };
    let own = {
        let text = text.clone();
        Memo::new(move |_| {
            session
                .needles
                .with(|list| list.iter().any(|n| n.text == text && n.label.is_some()))
        })
    };
    let set = {
        let text = text.clone();
        move |label: Option<String>| {
            session.needles.update(|list| {
                if let Some(needle) = list.iter_mut().find(|n| n.text == text) {
                    needle.label = label;
                }
            });
        }
    };
    let commit = {
        let (text, set) = (text.clone(), set.clone());
        move || {
            if !editing.get_untracked() {
                return;
            }
            editing.set(false);
            let typed = draft.get_untracked().trim().to_string();
            let computed = session
                .labels
                .with_untracked(|labels| labels.get(&text).cloned());
            set((computed.as_ref() != Some(&typed)).then_some(typed));
        }
    };
    let open = move |_| {
        draft.set(shown.get_untracked());
        editing.set(true);
    };

    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        if let Some(input) = field.get() {
            input.set_value(&draft.get_untracked());
            let _ = input.focus();
            input.select();
        }
    });

    let chip = move || {
        let label = shown.get();
        let tone = match own.get() {
            true => "border-[var(--color-accent)] text-[var(--color-accent)]",
            false => "border-[var(--color-rule)] text-[var(--color-ink-muted)]",
        };
        let (face, words) = match label.is_empty() {
            true => ("italic", ERASED.to_string()),
            false => ("", label),
        };
        view! {
            <button
                type="button"
                class=format!("{TAG} {tone} {face}")
                title="Modifier l'étiquette"
                on:click=open
            >
                {words}
            </button>
        }
    };
    let on_key = {
        let commit = commit.clone();
        move |event: leptos::ev::KeyboardEvent| match event.key().as_str() {
            "Enter" => {
                event.prevent_default();
                commit();
            }
            "Escape" => editing.set(false),
            _ => {}
        }
    };
    let reset = move |_| {
        editing.set(false);
        set(None);
    };
    let auto = move || {
        let reset = reset.clone();
        own.get().then(|| {
            view! {
                <button
                    type="button"
                    class="shrink-0 text-xs text-[var(--color-accent)] hover:underline"
                    title="Revenir à l'étiquette proposée"
                    on:mousedown=|event| event.prevent_default()
                    on:click=reset
                >
                    "auto"
                </button>
            }
        })
    };

    view! {
        <Show when=move || editing.get() fallback=chip>
            <input
                node_ref=field
                type="text"
                aria-label="Étiquette"
                class="h-5 w-28 shrink-0 rounded-sm border border-[var(--color-rule)] bg-[var(--color-parchment)] px-1.5 font-mono text-xs text-[var(--color-ink)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)]"
                prop:value=move || draft.get()
                on:input=move |event| draft.set(event_target_value(&event))
                on:keydown=on_key.clone()
                on:blur={
                    let commit = commit.clone();
                    move |_| commit()
                }
            />
            {auto.clone()}
        </Show>
    }
}

#[component]
fn Family(session: Session, kind: &'static str, label: &'static str) -> impl IntoView {
    let rows = move || {
        session.needles.with(|list| {
            list.iter()
                .filter(|needle| needle.kind == kind)
                .map(|needle| needle.text.clone())
                .collect::<Vec<_>>()
        })
    };

    view! {
        <Show when=move || !rows().is_empty()>
            <div class="flex flex-col gap-1">
                <p class=SECTION>{label}</p>
                {move || {
                    rows()
                        .into_iter()
                        .map(|text| view! { <NeedleRow session=session text=text /> })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}

/// Where the text reader is, in the words the reader is shown.
fn stage(reading: Option<Reading>) -> Option<String> {
    let Some(Reading::Stage(stage, at)) = reading else {
        return None;
    };
    let named = match stage.as_str() {
        "loading tesseract core" | "initializing tesseract" => "chargement du lecteur",
        "loading language traineddata" | "initializing api" | "initialized api" => {
            "chargement du français"
        }
        "loading models" => "chargement du lecteur",
        "recognizing text" => "lecture des pages",
        other => other,
    };
    Some(format!("{named} {} %", (at * 100.0).round()))
}

/// Offered only when the file yields no text of its own, which is what a scan
/// looks like from here.
#[component]
fn Reader(session: Session) -> impl IntoView {
    let read = move |eye: Eye| {
        move |_| {
            #[cfg(feature = "hydrate")]
            engine::read_pages(session, eye);
            #[cfg(not(feature = "hydrate"))]
            let _ = eye;
        }
    };
    let running = move || stage(session.reading.get());
    let failed = move || match session.reading.get() {
        Some(Reading::Failed(why)) => Some(why),
        _ => None,
    };
    let button = "h-8 flex-1 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 text-sm text-[var(--color-ink)] hover:bg-[var(--color-vellum)] disabled:pointer-events-none disabled:opacity-40";

    view! {
        <div class="flex flex-col gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)] p-2">
            <p class="px-1 text-sm leading-relaxed text-[var(--color-ink-muted)]">
                "Utiliser la reconnaissance de caractères :"
            </p>
            {move || failed().map(|why| view! {
                <p class="px-1 text-sm leading-relaxed text-[var(--color-accent)]">
                    {format!("La lecture s'est arrêtée : {why}")}
                </p>
            })}
            <Show
                when=move || running().is_none()
                fallback=move || {
                    view! {
                        <p class="px-1 text-sm tabular-nums text-[var(--color-ink-muted)]">{running}</p>
                    }
                }
            >
                <div class="flex items-center gap-2">
                    <button
                        type="button"
                        class=button
                        disabled=move || session.busy.get()
                        title="1,4 Mo"
                        on:click=read(Eye::Fast)
                    >
                        "Rapide"
                    </button>
                    <button
                        type="button"
                        class=button
                        disabled=move || session.busy.get()
                        title="Lit aussi l'écriture à la main, 13 Mo"
                        on:click=read(Eye::Slow)
                    >
                        "Performant"
                    </button>
                </div>
            </Show>
        </div>
    }
}

#[component]
pub fn Sidebar(session: Session) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let ticked = move || session.active().len();
    let drawn = move || session.rects.with(Vec::len);
    let ready = move || !session.busy.get() && (ticked() > 0 || drawn() > 0);

    let on_submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        session.add_needle(&draft.get_untracked(), "manual");
        draft.set(String::new());
    };
    let on_apply = move |_| {
        #[cfg(feature = "hydrate")]
        engine::apply(session);
    };
    let outlets = StoredValue::new(expect_context::<Outlets>().0);
    let fire = move |at: usize| {
        outlets.with_value(|list| {
            list.iter()
                .for_each(|outlet| outlet.state.set(Handover::Idle));
            list[at].fire();
        })
    };
    let held = RwSignal::new(None::<usize>);
    let send = move |at: usize| match session.stale() {
        true => held.set(Some(at)),
        false => fire(at),
    };
    let on_send_anyway = move |_| {
        if let Some(at) = held.get_untracked() {
            held.set(None);
            fire(at);
        }
    };
    let on_apply_again = move |_| {
        held.set(None);
        #[cfg(feature = "hydrate")]
        engine::apply(session);
    };
    Effect::new(move |_| {
        session.generation.track();
        outlets.with_value(|list| {
            list.iter()
                .for_each(|outlet| outlet.state.set(Handover::Idle))
        });
    });
    let sending =
        move || outlets.with_value(|list| list.iter().any(|o| o.state.get() == Handover::Busy));
    let said = move || {
        outlets.with_value(|list| {
            list.iter().find_map(|outlet| match outlet.state.get() {
                Handover::Idle => None,
                Handover::Busy => Some((false, "Un instant...".to_string())),
                Handover::Done(said) => Some((false, said.to_string())),
                Handover::Failed(why) => Some((true, format!("L'envoi s'est arrêté : {why}"))),
            })
        })
    };

    let waiting = move || {
        let _ = session.verdict.get();
        #[cfg(feature = "hydrate")]
        return engine::produced();
        #[cfg(not(feature = "hydrate"))]
        return false;
    };

    view! {
        <aside class="order-last flex max-h-[45%] min-h-0 w-full shrink-0 flex-col border-t border-[var(--color-rule)] bg-[var(--color-parchment)] md:order-none md:max-h-none md:w-80 md:border-r md:border-t-0">
            <div class="flex flex-col gap-2 border-b border-[var(--color-rule)] p-3">
                <form class="flex gap-2" on:submit=on_submit>
                    <input
                        type="text"
                        placeholder="Un nom, une adresse, un numéro"
                        aria-label="Ajouter un texte à retirer"
                        class="h-9 min-w-0 flex-1 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2 text-sm text-[var(--color-ink)] placeholder:text-[var(--color-ink-subtle)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)]"
                        prop:value=move || draft.get()
                        on:input=move |event| draft.set(event_target_value(&event))
                    />
                    <button
                        type="submit"
                        class="h-9 shrink-0 rounded-md border border-[var(--color-rule)] px-3 text-sm text-[var(--color-ink)] hover:bg-[var(--color-vellum)]"
                    >
                        "Ajouter"
                    </button>
                </form>
            </div>

            <div class="flex-1 overflow-auto p-3">
                <Show when=move || session.to_read.with(|pages| !pages.is_empty())>
                    <div class="mb-4">
                        <Reader session=session />
                    </div>
                </Show>
                <Show
                    when=move || session.needles.with(|list| !list.is_empty())
                    fallback=move || {
                        view! {
                            <p class="p-2 text-sm leading-relaxed text-[var(--color-ink-muted)]">
                                {move || match session.format.get().has_pixels() {
                                    true => "Rien à retirer pour l'instant. Tapez ce qui doit disparaître, ou tracez un rectangle sur la page.",
                                    false => "Rien à retirer pour l'instant. Tapez ce qui doit disparaître.",
                                }}
                            </p>
                        }
                    }
                >
                    <div class="flex flex-col gap-4">
                        {FAMILIES
                            .iter()
                            .map(|(kind, label)| {
                                view! { <Family session=session kind=*kind label=*label /> }
                            })
                            .collect_view()}
                    </div>
                </Show>

                <Show when=move || { drawn() > 0 }>
                    <div class="mt-4 flex flex-col gap-1 border-t border-[var(--color-rule)] pt-3">
                        <div class="flex items-center justify-between">
                            <p class=SECTION>"Rectangles"</p>
                            <button
                                type="button"
                                class="text-xs text-[var(--color-accent)] hover:underline"
                                on:click=move |_| session.rects.set(Vec::new())
                            >
                                "Tout effacer"
                            </button>
                        </div>
                        <p class="text-sm text-[var(--color-ink-muted)]">
                            {move || format!("{} tracé(s). Les pixels dessous partent aussi.", drawn())}
                        </p>
                    </div>
                </Show>
            </div>

            <div class="flex flex-col gap-3 border-t border-[var(--color-rule)] p-3">
                {move || {
                    session.verdict.get().map(|verdict| view! { <Panel verdict=verdict /> })
                }}
                <p class="text-xs text-[var(--color-ink-subtle)]">
                    {move || {
                        format!("{} chaîne(s) cochée(s), {} rectangle(s)", ticked(), drawn())
                    }}
                </p>
                <div class="flex gap-2 md:flex-col-reverse md:gap-3">
                    <button
                        type="button"
                        class="h-11 min-w-0 flex-1 truncate rounded-md md:w-full md:flex-none bg-[var(--color-ink)] px-4 text-sm font-medium text-[var(--color-parchment)] transition-colors hover:bg-[var(--color-ink-muted)] disabled:pointer-events-none disabled:opacity-40"
                        disabled=move || !ready()
                        on:click=on_apply
                    >
                        {move || match session.busy.get() {
                            true => {
                                stage(session.reading.get())
                                    .map(|at| format!("Vérification : {at}"))
                                    .unwrap_or_else(|| "Retrait en cours...".to_string())
                            }
                            false => "Anonymiser".to_string(),
                        }}
                    </button>
                    {move || {
                        waiting()
                            .then(|| {
                                outlets
                                    .get_value()
                                    .into_iter()
                                    .enumerate()
                                    .map(|(at, outlet)| {
                                        let tone = match at {
                                            0 => FOREMOST,
                                            _ => ASIDE,
                                        };
                                        view! {
                                            <button
                                                type="button"
                                                class=format!("{OUTLET} {tone}")
                                                disabled=move || session.busy.get() || sending()
                                                on:click=move |_| send(at)
                                            >
                                                {outlet.label}
                                            </button>
                                        }
                                    })
                                    .collect_view()
                            })
                    }}
                </div>
                {move || {
                    said()
                        .map(|(failed, text)| {
                            let tone = match failed {
                                true => "text-[var(--color-accent)]",
                                false => "text-[var(--color-ink-muted)]",
                            };
                            view! {
                                <p role="status" class=format!("text-sm leading-relaxed {tone}")>
                                    {text}
                                </p>
                            }
                        })
                }}
            </div>
            <Show when=move || held.get().is_some()>
                <div
                    class="fixed inset-0 z-50 flex items-center justify-center bg-[var(--color-ink)]/40 p-4"
                    on:click=move |_| held.set(None)
                >
                    <div
                        role="alertdialog"
                        aria-modal="true"
                        aria-labelledby="redact-stale-title"
                        class="flex w-full max-w-md flex-col gap-3 rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)] p-5 shadow-lg"
                        on:click=|event| event.stop_propagation()
                    >
                        <p id="redact-stale-title" class="font-sans text-base font-medium text-[var(--color-ink)]">
                            "Ce fichier ne contient pas vos dernières modifications"
                        </p>
                        <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                            "Anonymisez à nouveau pour y inclure les rectangles et les cases cochées depuis."
                        </p>
                        <div class="mt-2 flex flex-wrap justify-end gap-2">
                            <button
                                type="button"
                                class="h-9 rounded-md border border-[var(--color-rule)] px-3 text-sm text-[var(--color-ink)] hover:bg-[var(--color-vellum)]"
                                on:click=on_send_anyway
                            >
                                {move || {
                                    let label = held
                                        .get()
                                        .and_then(|at| outlets.with_value(|list| list.get(at).map(|o| o.label)))
                                        .unwrap_or_default();
                                    format!("{label} quand même")
                                }}
                            </button>
                            <button
                                type="button"
                                class="h-9 rounded-md bg-[var(--color-ink)] px-3 text-sm font-medium text-[var(--color-parchment)] hover:bg-[var(--color-ink-muted)] disabled:pointer-events-none disabled:opacity-40"
                                disabled=move || session.busy.get() || !ready()
                                on:click=on_apply_again
                            >
                                "Anonymiser à nouveau"
                            </button>
                        </div>
                    </div>
                </div>
            </Show>
        </aside>
    }
}

/// Said only when there is something to act on: a file withheld, or pages
/// still being read again.
#[component]
fn Panel(verdict: Verdict) -> impl IntoView {
    let withheld = match (&verdict.check, verdict.leaks.is_empty()) {
        (_, false) => Some(format!("Encore lisible : {}", verdict.leaks.join(", "))),
        (Check::Failed(why), _) => Some(format!("La relecture des pages s'est arrêtée : {why}")),
        _ => None,
    };
    match (withheld, verdict.check) {
        (Some(detail), _) => Either::Left(view! {
            <div class="flex flex-col gap-1 rounded-md border border-[var(--color-accent)] bg-[var(--color-accent-soft)] p-3 text-sm text-[var(--color-accent)]">
                <p class="font-medium">"Le fichier anonymisé n'est pas téléchargeable."</p>
                <p class="leading-relaxed">{detail}</p>
            </div>
        }),
        (None, Check::Running) => Either::Right(Some(view! {
            <p class="text-xs text-[var(--color-ink-subtle)]">"Vérification des pages..."</p>
        })),
        (None, _) => Either::Right(None),
    }
}
