//! `DateRangePicker` and `DatePicker`: one month-at-a-time calendar in plain
//! HTML/CSS, over civil date arithmetic of our own. Dates are ISO `YYYY-MM-DD`.

use leptos::prelude::*;

#[cfg(feature = "hydrate")]
use super::panel::{dismiss, Dismiss};
use super::{DropdownSelect, SelectOption};
use crate::helpers::cn;
use crate::i18n::DatesUi;

// ── Date civile minimale ─────────────────────────────────────────────────────

/// `(année, mois 1-12, jour 1-31)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Civil {
    y: i32,
    m: u32,
    d: u32,
}

const START_YEAR: i32 = 1965;

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Jour de semaine (0 = lundi … 6 = dimanche). Zeller adapté.
fn weekday_monday0(c: Civil) -> u32 {
    let (mut y, mut m) = (c.y, c.m as i32);
    if m < 3 {
        m += 12;
        y -= 1;
    }
    let k = y % 100;
    let j = y / 100;
    // Zeller : 0 = samedi.
    let h = (c.d as i32 + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    // Convertit (0=sam) -> (0=lun).
    ((h + 5) % 7) as u32
}

fn parse_iso(s: &str) -> Option<Civil> {
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    if (1..=12).contains(&m) && (1..=31).contains(&d) {
        Some(Civil { y, m, d })
    } else {
        None
    }
}

fn to_iso(c: Civil) -> String {
    format!("{:04}-{:02}-{:02}", c.y, c.m, c.d)
}

/// Sans accent ni point final, minuscules : « Févr. » et « fevrier » se
/// comparent au même mois.
fn fold_month(s: &str) -> String {
    lj_fold::fold(s.trim().trim_end_matches('.'))
}

/// Mois nommé, sur les seules formes que nous affichons. Un préfixe partiel ne
/// compte pas : « jui » n'a pas de réponse.
fn month_from_name(ui: &DatesUi, token: &str) -> Option<u32> {
    let t = fold_month(token);
    if t.len() < 3 {
        return None;
    }
    ui.months
        .iter()
        .chain(ui.months_short.iter())
        .position(|m| fold_month(m) == t)
        .map(|i| (i % 12) as u32 + 1)
}

/// What a typed entry designates: a day, a whole month or a whole year. The
/// field resolves the last two on its own bound ([`Typed::resolve`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Typed {
    Day(Civil),
    Month { y: i32, m: u32 },
    Year(i32),
}

impl Typed {
    /// `Du` takes the first day of the span, `Au` the last one.
    fn resolve(self, bound: Focus) -> Civil {
        match (self, bound) {
            (Typed::Day(c), _) => c,
            (Typed::Month { y, m }, Focus::From) => Civil { y, m, d: 1 },
            (Typed::Month { y, m }, Focus::To) => Civil {
                y,
                m,
                d: days_in_month(y, m),
            },
            (Typed::Year(y), Focus::From) => Civil { y, m: 1, d: 1 },
            (Typed::Year(y), Focus::To) => Civil { y, m: 12, d: 31 },
        }
    }
}

/// A year is four digits, no less: while the year of `1 janv. 2023` is being
/// edited, `202` is not a date and nothing must leave the field.
fn year_token(t: &str) -> Option<i32> {
    if t.len() != 4 || !t.bytes().all(|b| b.is_ascii_digit()) || t.starts_with('0') {
        return None;
    }
    t.parse().ok()
}

fn month_token(ui: &DatesUi, t: &str) -> Option<u32> {
    if let Some(m) = month_from_name(ui, t) {
        return Some(m);
    }
    if t.len() > 2 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse().ok().filter(|m| (1..=12).contains(m))
}

/// A hand-typed entry: `12/03/2024`, `12-03-2024`, `12032024`, the ISO
/// `2024-03-12`, and the form the field displays, `12 mars 2024`. A month alone
/// (`03/2024`, `mars 2024`) or a year alone (`2024`) designates its whole span.
/// Anything else yields `None`, and the entry stays as typed.
fn parse_typed(ui: &DatesUi, s: &str) -> Option<Typed> {
    let tokens: Vec<&str> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    match tokens[..] {
        [y] => {
            if let Some(y) = year_token(y) {
                return Some(Typed::Year(y));
            }
        }
        [m, y] => {
            if let (Some(m), Some(y)) = (month_token(ui, m), year_token(y)) {
                return Some(Typed::Month { y, m });
            }
        }
        [d, name, y] => {
            if let (Some(m), Some(y)) = (month_from_name(ui, name), year_token(y)) {
                let d = match d {
                    "1er" => 1,
                    d => d.parse::<u32>().ok()?,
                };
                return (1..=days_in_month(y, m))
                    .contains(&d)
                    .then_some(Typed::Day(Civil { y, m, d }));
            }
        }
        _ => {}
    }
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 8 {
        return None;
    }
    let num = |slice: &[u32]| slice.iter().fold(0i32, |acc, d| acc * 10 + *d as i32);
    // Un groupe de 4 chiffres en tête ne peut être qu'une année.
    let (y, m, d) = if s.trim_start().starts_with(|c: char| c.is_ascii_digit())
        && s.split(|c: char| !c.is_ascii_digit()).next()?.len() == 4
    {
        (num(&digits[0..4]), num(&digits[4..6]), num(&digits[6..8]))
    } else {
        (num(&digits[4..8]), num(&digits[2..4]), num(&digits[0..2]))
    };
    let (m, d) = (u32::try_from(m).ok()?, u32::try_from(d).ok()?);
    ((1..=12).contains(&m) && (1..=days_in_month(y, m)).contains(&d)).then_some(Typed::Day(Civil {
        y,
        m,
        d,
    }))
}

/// The range once `bound` takes `iso`. `None` when the bounds would end up in
/// the wrong order: an inverted range answers nothing to every search, so it
/// is emptied and starts over.
fn range_with(bound: Focus, iso: &str, from: &str, to: &str) -> Option<(String, String)> {
    let (f, t) = match bound {
        Focus::From => (iso, to),
        Focus::To => (from, iso),
    };
    match (parse_iso(f), parse_iso(t)) {
        (Some(a), Some(b)) if a > b => None,
        _ => Some((f.to_string(), t.to_string())),
    }
}

fn format_short(ui: &DatesUi, c: Civil) -> String {
    (ui.short_date)(c.y, c.m, c.d)
}

/// Mois suivant.
fn next_month(y: i32, m: u32) -> (i32, u32) {
    if m == 12 {
        (y + 1, 1)
    } else {
        (y, m + 1)
    }
}

/// Année courante : JS `Date` en hydrate, const fallback en SSR.
#[cfg(feature = "hydrate")]
fn current_year() -> i32 {
    js_sys::Date::new_0().get_full_year() as i32
}
#[cfg(feature = "ssr")]
fn current_year() -> i32 {
    2026
}

/// Mois courant `(année, mois 1-12)` : ouverture du calendrier sur le mois en
/// cours, pas sur janvier.
#[cfg(feature = "hydrate")]
fn current_ym() -> (i32, u32) {
    let d = js_sys::Date::new_0();
    (d.get_full_year() as i32, d.get_month() + 1)
}
#[cfg(feature = "ssr")]
fn current_ym() -> (i32, u32) {
    (2026, 6)
}

// ── Grille d'un mois ─────────────────────────────────────────────────────────

/// Cellule de la grille mensuelle.
#[derive(Debug, Clone, Copy)]
struct Cell {
    date: Civil,
    outside: bool,
}

/// Grille 6×7 lundi-first incluant les jours « outside » (mois adjacents).
fn month_grid(y: i32, m: u32) -> Vec<Cell> {
    let first = Civil { y, m, d: 1 };
    let lead = weekday_monday0(first); // nb de jours du mois précédent à afficher
    let dim = days_in_month(y, m);
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let pdim = days_in_month(py, pm);
    let (ny, nm) = next_month(y, m);

    let mut cells = Vec::with_capacity(42);
    // Jours du mois précédent.
    for i in 0..lead {
        let d = pdim - lead + 1 + i;
        cells.push(Cell {
            date: Civil { y: py, m: pm, d },
            outside: true,
        });
    }
    // Jours du mois courant.
    for d in 1..=dim {
        cells.push(Cell {
            date: Civil { y, m, d },
            outside: false,
        });
    }
    // Complète à 42 (6 semaines) avec le mois suivant.
    let mut nd = 1;
    while cells.len() < 42 {
        cells.push(Cell {
            date: Civil {
                y: ny,
                m: nm,
                d: nd,
            },
            outside: true,
        });
        nd += 1;
    }
    cells
}

// ── Composant ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    From,
    To,
}

#[component]
pub fn DateRangePicker(
    #[prop(into)] from: Signal<String>,
    #[prop(into)] to: Signal<String>,
    #[prop(into)] on_change: Callback<(String, String)>,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().dates;
    let from_date = Signal::derive(move || parse_iso(&from.get()));
    let to_date = Signal::derive(move || parse_iso(&to.get()));

    let focus = RwSignal::new(
        if !from.get_untracked().is_empty() && to.get_untracked().is_empty() {
            Focus::To
        } else {
            Focus::From
        },
    );
    let calendar_open = RwSignal::new(false);

    // Mois affiché. Initialisé sur `from`, sinon le mois courant.
    let init_month = from_date
        .get_untracked()
        .map(|c| (c.y, c.m))
        .unwrap_or_else(current_ym);
    let view_month = RwSignal::new(init_month);
    let end_year = current_year();

    // A bound that crosses the other empties both, and the range starts over
    // from `Du`.
    let apply = move |bound: Focus, iso: &str| -> bool {
        match range_with(bound, iso, &from.get_untracked(), &to.get_untracked()) {
            Some(next) => {
                on_change.run(next);
                true
            }
            None => {
                on_change.run((String::new(), String::new()));
                focus.set(Focus::From);
                false
            }
        }
    };

    let select_day = move |c: Civil| {
        let iso = to_iso(c);
        match focus.get_untracked() {
            Focus::From => {
                if apply(Focus::From, &iso) {
                    focus.set(Focus::To);
                }
            }
            Focus::To => {
                if apply(Focus::To, &iso) {
                    calendar_open.set(false);
                }
            }
        }
    };

    let field = move |label: &'static str, value: Signal<String>, this: Focus| {
        // `draft` = la frappe en cours. Tant qu'elle vit, elle prime sur la forme
        // canonique, sinon reformater sous les doigts empêche de finir la saisie.
        let draft = RwSignal::new(None::<String>);
        let active = move || calendar_open.get() && focus.get() == this;
        let emit = move |iso: &str| {
            apply(this, iso);
        };
        let on_activate = move |_| {
            focus.set(this);
            calendar_open.set(true);
        };
        let on_input = move |ev: leptos::ev::Event| {
            let raw = event_target_value(&ev);
            if raw.trim().is_empty() {
                draft.set(None);
                emit("");
                return;
            }
            draft.set(Some(raw.clone()));
            if let Some(c) = parse_typed(ui, &raw).map(|t| t.resolve(this)) {
                let iso = to_iso(c);
                if iso != value.get_untracked() {
                    view_month.set((c.y, c.m));
                    emit(&iso);
                }
            }
        };
        let on_blur = move |_| draft.set(None);
        let on_key = move |ev: leptos::ev::KeyboardEvent| {
            if ev.key() == "Enter" {
                ev.prevent_default();
                draft.set(None);
                calendar_open.set(false);
            }
        };
        let on_clear = move |_| {
            draft.set(None);
            emit("");
            focus.set(this);
            calendar_open.set(false);
        };
        let shown = move || match draft.get() {
            Some(text) => text,
            None => parse_iso(&value.get())
                .map(|c| format_short(ui, c))
                .unwrap_or_default(),
        };
        // `min-w-0` : sans lui la largeur minimale du champ est celle que
        // l'`<input>` se donne, 181 px, et les deux débordent le panneau de 320.
        let class = move || {
            cn([
                "flex min-w-0 flex-1 items-center justify-between gap-1 rounded border px-2 py-1.5 text-xs transition-colors",
                if active() {
                    "border-[var(--color-accent)] bg-[var(--color-bordeaux-soft)]"
                } else {
                    "border-[var(--color-rule)] hover:border-[var(--color-ink-muted)]"
                },
            ])
        };
        view! {
            <div class=class>
                <input
                    type="text"
                    inputmode="numeric"
                    autocomplete="off"
                    aria-label=(ui.typed_label)(label)
                    placeholder=format!("{label} {}", ui.date_format)
                    prop:value=shown
                    on:focus=on_activate
                    on:input=on_input
                    on:blur=on_blur
                    on:keydown=on_key
                    class="min-w-0 flex-1 bg-transparent text-xs text-[var(--color-ink)] outline-none placeholder:text-[var(--color-ink-muted)] placeholder:opacity-50"
                />
                <Show when=move || !value.get().is_empty()>
                    <button
                        type="button"
                        aria-label=(ui.clear)(label)
                        on:click=on_clear
                        class="shrink-0 opacity-60 hover:opacity-100"
                    >
                        "✕"
                    </button>
                </Show>
            </div>
        }
    };

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex gap-1.5">
                {field(ui.from, from, Focus::From)} {field(ui.to, to, Focus::To)}
            </div>
            <Show when=move || calendar_open.get()>
                {calendar(ui, view_month, (START_YEAR, end_year), from_date, to_date, select_day)}
            </Show>
        </div>
    }
}

/// One date, picked in a floating panel. A typed date is only highlighted
/// until Enter confirms it: `on_change` fires once per choice.
#[component]
pub fn DatePicker(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(into)] aria_label: String,
    first_year: i32,
    years_ahead: i32,
) -> impl IntoView {
    let ui = &crate::i18n::use_ui().dates;
    let open = RwSignal::new(false);
    let picked = Signal::derive(move || parse_iso(&value.get()));
    let pending = RwSignal::new(None::<Civil>);
    let shown = Signal::derive(move || pending.get().or_else(|| picked.get()));
    let draft = RwSignal::new(None::<String>);
    let view_month = RwSignal::new(current_ym());
    let last_year = current_year() + years_ahead;
    let coords = RwSignal::new(None::<(f64, f64, f64)>);
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let button_ref = NodeRef::<leptos::html::Button>::new();
    let input_ref = NodeRef::<leptos::html::Input>::new();

    let close = move || {
        open.set(false);
        draft.set(None);
        pending.set(None);
    };

    #[cfg(feature = "hydrate")]
    dismiss(
        container_ref,
        move || open.get_untracked(),
        close,
        Dismiss {
            escape: true,
            scroll: true,
        },
    );

    let commit = move |c: Civil| {
        close();
        let iso = to_iso(c);
        if iso != value.get_untracked() {
            on_change.run(iso);
        }
    };

    let on_toggle = move |_| {
        if open.get_untracked() {
            close();
            return;
        }
        #[cfg(feature = "hydrate")]
        if let Some(button) = button_ref.get_untracked() {
            const PANEL_W: f64 = 320.0;
            let rect = button.get_bounding_client_rect();
            let dimension = |v: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
                v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
            };
            let win_h = dimension(window().inner_height());
            let win_w = dimension(window().inner_width());
            let top = rect.bottom() + 4.0;
            let left = rect.left().min(win_w - PANEL_W - 8.0).max(8.0);
            let max_h = (win_h - top - 16.0).clamp(160.0, 480.0);
            coords.set(Some((top, left, max_h)));
        }
        view_month.set(
            picked
                .get_untracked()
                .map(|c| (c.y, c.m))
                .unwrap_or_else(current_ym),
        );
        open.set(true);
        request_animation_frame(move || {
            if let Some(input) = input_ref.get_untracked() {
                let _ = input.focus();
            }
        });
    };

    let on_input = move |ev: leptos::ev::Event| {
        let raw = event_target_value(&ev);
        let typed = parse_typed(ui, &raw).map(|t| t.resolve(Focus::From));
        draft.set(Some(raw));
        pending.set(typed);
        if let Some(c) = typed {
            view_month.set((c.y, c.m));
        }
    };
    let on_key = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            ev.prevent_default();
            if let Some(c) = pending.get_untracked() {
                commit(c);
            }
        }
    };
    let field_text = move || {
        draft.get().unwrap_or_else(|| {
            picked
                .get()
                .map(|c| format_short(ui, c))
                .unwrap_or_default()
        })
    };

    let panel_style = move || match coords.get() {
        Some((top, left, max_h)) => {
            format!("position:fixed; top:{top}px; left:{left}px; max-height:{max_h}px;")
        }
        None => "position:fixed; top:0; left:0;".to_string(),
    };

    view! {
        <div node_ref=container_ref class="relative inline-flex max-w-full">
            <button
                node_ref=button_ref
                type="button"
                aria-label=aria_label
                aria-haspopup="dialog"
                aria-expanded=move || open.get().then_some("true")
                on:click=on_toggle
                class="flex min-h-9 items-center gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-1.5 text-left text-sm text-[var(--color-ink)] transition-colors hover:border-[var(--color-ink)]"
            >
                <span>{move || picked.get().map(|c| format_short(ui, c)).unwrap_or_default()}</span>
                <svg
                    viewBox="0 0 12 8"
                    class=move || {
                        format!(
                            "h-2 w-2.5 shrink-0 text-[var(--color-ink-subtle)] transition-transform {}",
                            if open.get() { "rotate-180" } else { "" },
                        )
                    }
                    fill="none"
                    aria-hidden="true"
                >
                    <path
                        d="M1 1.5l5 5 5-5"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    />
                </svg>
            </button>
            <div
                style=panel_style
                data-open=move || open.get().then_some("true")
                class="lj-panel z-50 flex w-80 flex-col gap-3 overflow-y-auto overscroll-contain rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-3 shadow-lg"
            >
                <Show when=move || open.get()>
                    <input
                        node_ref=input_ref
                        type="text"
                        inputmode="numeric"
                        autocomplete="off"
                        aria-label=(ui.typed_label)(ui.date)
                        placeholder=ui.date_format
                        prop:value=field_text
                        on:input=on_input
                        on:keydown=on_key
                        class="w-full rounded border border-[var(--color-rule)] bg-transparent px-2 py-1.5 text-xs text-[var(--color-ink)] outline-none transition-colors placeholder:text-[var(--color-ink-muted)] placeholder:opacity-50 focus:border-[var(--color-accent)]"
                    />
                    {calendar(ui, view_month, (first_year, last_year), shown, shown, commit)}
                </Show>
            </div>
        </div>
    }
}

/// Arrows and month and year menus over the grid of `view_month`, which they
/// move within `years`.
fn calendar(
    ui: &'static DatesUi,
    view_month: RwSignal<(i32, u32)>,
    years: (i32, i32),
    from_date: Signal<Option<Civil>>,
    to_date: Signal<Option<Civil>>,
    select_day: impl Fn(Civil) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (first_year, last_year) = years;
    let go_prev = move |_| {
        view_month.update(|(y, m)| {
            if *m == 1 {
                *y -= 1;
                *m = 12;
            } else {
                *m -= 1;
            }
        });
    };
    let go_next = move |_| {
        view_month.update(|(y, m)| {
            let (ny, nm) = next_month(*y, *m);
            *y = ny;
            *m = nm;
        });
    };

    let at_start = move || view_month.get() <= (first_year, 1);
    let at_end = move || view_month.get() >= (last_year, 12);

    let month_options = Signal::derive(|| {
        ui.months
            .iter()
            .enumerate()
            .map(|(i, name)| SelectOption {
                value: (i + 1).to_string(),
                label: (*name).to_string(),
            })
            .collect::<Vec<_>>()
    });
    let year_options = Signal::derive(move || {
        (first_year..=last_year)
            .rev()
            .map(|y| SelectOption {
                value: y.to_string(),
                label: y.to_string(),
            })
            .collect::<Vec<_>>()
    });
    let month_value = Signal::derive(move || view_month.get().1.to_string());
    let year_value = Signal::derive(move || view_month.get().0.to_string());
    let on_pick_month = move |v: String| {
        if let Ok(m) = v.parse::<u32>() {
            view_month.update(|(_, vm)| *vm = m);
        }
    };
    let on_pick_year = move |v: String| {
        if let Ok(y) = v.parse::<i32>() {
            view_month.update(|(vy, _)| *vy = y);
        }
    };

    view! {
        <div class="flex flex-col gap-3">
            <div class="mb-1 flex items-center justify-between gap-1">
                <NavBtn direction="prev" disabled=Signal::derive(at_start) on_click=go_prev />
                <div class="flex items-center gap-1">
                    <DropdownSelect
                        value=month_value
                        on_change=Callback::new(on_pick_month)
                        options=month_options
                        aria_label=ui.month
                    />
                    <DropdownSelect
                        value=year_value
                        on_change=Callback::new(on_pick_year)
                        options=year_options
                        aria_label=ui.year
                    />
                </div>
                <NavBtn direction="next" disabled=Signal::derive(at_end) on_click=go_next />
            </div>
            <div>
                {move || {
                    let (y, m) = view_month.get();
                    month_view(ui, y, m, from_date, to_date, select_day)
                }}
            </div>
        </div>
    }
}

/// Rend une grille mensuelle (en-têtes + jours), modifiers range appliqués.
fn month_view(
    ui: &DatesUi,
    y: i32,
    m: u32,
    from_date: Signal<Option<Civil>>,
    to_date: Signal<Option<Civil>>,
    select_day: impl Fn(Civil) + Copy + 'static,
) -> impl IntoView {
    let cells = month_grid(y, m);
    view! {
        <table class="w-full border-collapse">
            <thead>
                <tr>
                    {ui
                        .weekdays
                        .iter()
                        .map(|wd| {
                            view! {
                                <th class="pb-1.5 text-center text-[0.68rem] uppercase tracking-wider text-[var(--color-ink-subtle)]">
                                    {*wd}
                                </th>
                            }
                        })
                        .collect::<Vec<_>>()}
                </tr>
            </thead>
            <tbody>
                {cells
                    .chunks(7)
                    .map(|week| {
                        let days = week
                            .iter()
                            .copied()
                            .map(|cell| {
                                day_cell(cell, from_date, to_date, select_day)
                            })
                            .collect::<Vec<_>>();
                        view! { <tr>{days}</tr> }
                    })
                    .collect::<Vec<_>>()}
            </tbody>
        </table>
    }
}

/// Rend une cellule-jour avec ses modifiers de plage.
fn day_cell(
    cell: Cell,
    from_date: Signal<Option<Civil>>,
    to_date: Signal<Option<Civil>>,
    select_day: impl Fn(Civil) + Copy + 'static,
) -> impl IntoView {
    let date = cell.date;
    let outside = cell.outside;
    let wrapper_class = move || {
        let f = from_date.get();
        let t = to_date.get();
        let is_start = f == Some(date);
        let is_end = t == Some(date);
        let in_range = matches!((f, t), (Some(a), Some(b)) if date > a && date < b);
        if is_start && is_end {
            "p-0 text-center bg-[var(--color-accent)] rounded-sm"
        } else if is_start {
            "p-0 text-center bg-[var(--color-accent)] rounded-l-sm"
        } else if is_end {
            "p-0 text-center bg-[var(--color-accent)] rounded-r-sm"
        } else if in_range {
            "p-0 text-center rounded-none bg-[var(--color-bordeaux-soft)]"
        } else {
            "p-0 text-center"
        }
    };
    let button_class = move || {
        let f = from_date.get();
        let t = to_date.get();
        let selected = f == Some(date) || t == Some(date);
        let in_range = matches!((f, t), (Some(a), Some(b)) if date > a && date < b);
        cn([
            "mx-auto flex h-7 w-7 items-center justify-center rounded-sm text-xs transition-colors hover:bg-[var(--color-bordeaux-soft)] hover:text-[var(--color-accent)]",
            if selected {
                "text-white"
            } else if in_range {
                "text-[var(--color-accent)]"
            } else {
                "text-[var(--color-ink-muted)]"
            },
            if outside { "opacity-30" } else { "" },
        ])
    };
    view! {
        <td class=wrapper_class>
            <button type="button" on:click=move |_| select_day(date) class=button_class>
                {date.d}
            </button>
        </td>
    }
}

#[component]
fn NavBtn(
    direction: &'static str,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(into)] on_click: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    let d = if direction == "prev" {
        "M7.5 2L3.5 6l4 4"
    } else {
        "M4.5 2l4 4-4 4"
    };
    view! {
        <button
            type="button"
            on:click=move |ev| on_click.run(ev)
            prop:disabled=move || disabled.get()
            class="flex h-6 w-6 shrink-0 items-center justify-center rounded border border-[var(--color-rule)] text-[var(--color-ink-muted)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)] disabled:pointer-events-none disabled:opacity-30"
        >
            <svg viewBox="0 0 12 12" class="h-3 w-3" fill="none" aria-hidden="true">
                <path
                    d=d
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            </svg>
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    fn parse_typed(s: &str) -> Option<Typed> {
        super::parse_typed(&Lang::Fr.ui().dates, s)
    }

    fn format_short(c: Civil) -> String {
        super::format_short(&Lang::Fr.ui().dates, c)
    }

    #[test]
    fn typed_date_accepts_the_forms_a_reader_writes() {
        let expected = Civil {
            y: 2024,
            m: 3,
            d: 12,
        };
        for s in ["12/03/2024", "12-03-2024", "12 03 2024", "12032024"] {
            assert_eq!(
                parse_typed(s),
                Some(Typed::Day(expected)),
                "forme refusée : {s}"
            );
        }
        // Un groupe de 4 chiffres en tête se lit comme une année.
        assert_eq!(parse_typed("2024-03-12"), Some(Typed::Day(expected)));
        // Incomplet, hors calendrier, ou bruité : pas de date.
        for s in [
            "12/03",
            "",
            "32/03/2024",
            "12/13/2024",
            "30/02/2024",
            "abcd",
        ] {
            assert_eq!(parse_typed(s), None, "forme acceptée à tort : {s}");
        }
        // Le 29 février n'existe qu'en année bissextile.
        assert!(parse_typed("29/02/2024").is_some());
        assert!(parse_typed("29/02/2023").is_none());
    }

    /// Le champ doit relire ce qu'il écrit, sinon retaper ce qu'on voit échoue.
    #[test]
    fn typed_date_reads_back_what_the_field_displays() {
        for (m, last) in (1u32..=12).zip([31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]) {
            for d in [1, last] {
                let c = Civil { y: 2019, m, d };
                assert_eq!(
                    parse_typed(&format_short(c)),
                    Some(Typed::Day(c)),
                    "{}",
                    format_short(c)
                );
            }
        }
        assert_eq!(
            parse_typed("5 juin 2019"),
            Some(Typed::Day(Civil {
                y: 2019,
                m: 6,
                d: 5
            }))
        );
        // Nom complet et sans accent, puisque c'est ce qu'un lecteur tape.
        assert_eq!(
            parse_typed("5 fevrier 2019"),
            Some(Typed::Day(Civil {
                y: 2019,
                m: 2,
                d: 5
            }))
        );
        // Un préfixe ambigu ne devine pas.
        assert_eq!(parse_typed("5 jui 2019"), None);
    }

    #[test]
    fn a_month_or_a_year_alone_spans_its_bound() {
        let iso = |s: &str, bound: Focus| parse_typed(s).map(|t| to_iso(t.resolve(bound)));
        assert_eq!(iso("2023", Focus::From).as_deref(), Some("2023-01-01"));
        assert_eq!(iso("2023", Focus::To).as_deref(), Some("2023-12-31"));
        assert_eq!(iso("02/2024", Focus::From).as_deref(), Some("2024-02-01"));
        assert_eq!(iso("02/2024", Focus::To).as_deref(), Some("2024-02-29"));
        assert_eq!(iso("mars 2023", Focus::To).as_deref(), Some("2023-03-31"));
        assert_eq!(iso("12/03/2024", Focus::To).as_deref(), Some("2024-03-12"));
    }

    #[test]
    fn a_bound_crossing_the_other_empties_the_range() {
        assert_eq!(range_with(Focus::To, "2026-01-01", "2026-01-05", ""), None);
        assert_eq!(
            range_with(Focus::From, "2026-01-09", "", "2026-01-05"),
            None
        );
        let same = Some(("2026-01-05".to_string(), "2026-01-05".to_string()));
        assert_eq!(range_with(Focus::To, "2026-01-05", "2026-01-05", ""), same);
        // Emptying a bound keeps the other one.
        let kept = Some((String::new(), "2026-01-09".to_string()));
        assert_eq!(
            range_with(Focus::From, "", "2026-01-05", "2026-01-09"),
            kept
        );
    }

    /// Editing the year of `1 janv. 2023` passes through `202` and `2`: none of
    /// those is a date, and nothing must leave the field meanwhile.
    #[test]
    fn a_partial_year_is_not_a_date() {
        for s in [
            "1 janv. 202",
            "1 janv. 20",
            "1 janv. 2",
            "202",
            "0202",
            "03/202",
            "13/2024",
        ] {
            assert_eq!(parse_typed(s), None, "forme acceptée à tort : {s}");
        }
    }

    #[test]
    fn weekday_known_dates() {
        assert_eq!(
            weekday_monday0(Civil {
                y: 2026,
                m: 6,
                d: 5
            }),
            4
        );
        assert_eq!(
            weekday_monday0(Civil {
                y: 2000,
                m: 1,
                d: 1
            }),
            5
        );
    }

    #[test]
    fn leap_year_february() {
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
    }

    #[test]
    fn grid_has_42_cells_and_starts_monday() {
        let g = month_grid(2026, 6);
        assert_eq!(g.len(), 42);
        assert!(!g[0].outside);
        assert_eq!(
            g[0].date,
            Civil {
                y: 2026,
                m: 6,
                d: 1
            }
        );
    }

    #[test]
    fn grid_leading_outside_days() {
        let g = month_grid(2026, 3);
        let lead = g.iter().take_while(|c| c.outside).count();
        assert_eq!(lead, 6);
        assert_eq!(
            g[6].date,
            Civil {
                y: 2026,
                m: 3,
                d: 1
            }
        );
    }

    #[test]
    fn iso_roundtrip() {
        let c = parse_iso("2024-08-06").unwrap();
        assert_eq!(
            c,
            Civil {
                y: 2024,
                m: 8,
                d: 6
            }
        );
        assert_eq!(to_iso(c), "2024-08-06");
    }
}
