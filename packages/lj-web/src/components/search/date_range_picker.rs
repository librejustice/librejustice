//! `DateRangePicker` (réécriture intégrale de `date-range-picker.tsx`).
//!
//! Le React utilise `react-day-picker` + `date-fns`. Cible : calendrier HTML/CSS
//! pur, mono-mois (le rail fait ~260px : deux mois côte à côte s'y chevauchent),
//! dates en arithmétique civile maison (pas de `jiff` : absent des dépendances
//! `lj-web`, Cargo figé). Zéro dépendance JS.
//!
//! Props : `from`/`to` ISO `YYYY-MM-DD` (ou vide), `on_change(from, to)`.

use leptos::prelude::*;

use crate::components::ui::{DropdownSelect, SelectOption};
use crate::helpers::cn;

// ── Date civile minimale ─────────────────────────────────────────────────────

/// `(année, mois 1-12, jour 1-31)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Civil {
    y: i32,
    m: u32,
    d: u32,
}

const MONTHS_FR: [&str; 12] = [
    "Janvier",
    "Février",
    "Mars",
    "Avril",
    "Mai",
    "Juin",
    "Juillet",
    "Août",
    "Septembre",
    "Octobre",
    "Novembre",
    "Décembre",
];
const MONTHS_FR_SHORT: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
    "déc.",
];
/// Jours de semaine FR (lundi-first), abrégés 2 lettres (parité React, tient en
/// largeur dans le rail étroit ; capitalisés par CSS `uppercase`).
const WEEKDAYS_FR: [&str; 7] = ["lu", "ma", "me", "je", "ve", "sa", "di"];
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

fn format_short(c: Civil) -> String {
    format!("{} {} {}", c.d, MONTHS_FR_SHORT[(c.m - 1) as usize], c.y)
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
/// cours (parité React), pas sur janvier.
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

    let select_day = move |c: Civil| {
        let iso = to_iso(c);
        match focus.get_untracked() {
            Focus::From => {
                on_change.run((iso.clone(), to.get_untracked()));
                if !iso.is_empty() {
                    focus.set(Focus::To);
                }
            }
            Focus::To => {
                on_change.run((from.get_untracked(), iso));
                calendar_open.set(false);
            }
        }
    };

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

    let at_start = move || view_month.get() <= (START_YEAR, 1);
    let at_end = move || view_month.get() >= (end_year, 12);

    // Menus déroulants mois + année : saut direct (sinon ~12 clics/an sur les
    // flèches pour reculer de plusieurs années). `DropdownSelect` (panneau thémé,
    // capé + scrollable, positionné `fixed` pour échapper à l'`overflow-hidden` de
    // l'accordéon du rail) plutôt que `<select>` natif, dont le popup hérite du fond
    // système et déborde (62 années).
    let month_options = Signal::derive(|| {
        MONTHS_FR
            .iter()
            .enumerate()
            .map(|(i, name)| SelectOption {
                value: (i + 1).to_string(),
                label: (*name).to_string(),
            })
            .collect::<Vec<_>>()
    });
    let year_options = Signal::derive(move || {
        (START_YEAR..=end_year)
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

    let field = move |label: &'static str, value: Signal<String>, this: Focus| {
        let parsed = move || parse_iso(&value.get());
        let active = move || calendar_open.get() && focus.get() == this;
        let on_activate = move |_| {
            focus.set(this);
            calendar_open.set(true);
        };
        // Le champ porte un bouton « effacer » : un `<button>` imbriqué dans un
        // `<button>` est du HTML invalide (le parseur referme le bouton externe
        // au bouton interne) → mismatch d'hydratation fatal côté SSR. Le champ est
        // donc un `div[role=button]` (clic + Enter/Espace) ; le bouton interne
        // reste valide. Parité visuelle/comportement avec le `<button>` Node.
        let on_activate_key = move |ev: leptos::ev::KeyboardEvent| {
            if ev.key() == "Enter" || ev.key() == " " {
                ev.prevent_default();
                focus.set(this);
                calendar_open.set(true);
            }
        };
        let on_clear = move |ev: leptos::ev::MouseEvent| {
            ev.stop_propagation();
            match this {
                Focus::From => on_change.run((String::new(), to.get_untracked())),
                Focus::To => on_change.run((from.get_untracked(), String::new())),
            }
            focus.set(this);
            calendar_open.set(false);
        };
        let class = move || {
            cn([
                "flex flex-1 cursor-pointer items-center justify-between gap-1 rounded border px-2 py-1.5 text-left text-xs transition-colors",
                if active() {
                    "border-[var(--color-accent)] bg-[var(--color-bordeaux-soft)] text-[var(--color-accent)]"
                } else {
                    "border-[var(--color-rule)] text-[var(--color-ink-muted)] hover:border-[var(--color-ink-muted)]"
                },
            ])
        };
        view! {
            <div role="button" tabindex="0" on:click=on_activate on:keydown=on_activate_key class=class>
                <span class="truncate">
                    {move || match parsed() {
                        Some(c) => view! { {format_short(c)} }.into_any(),
                        None => view! { <span class="opacity-50">{label}</span> }.into_any(),
                    }}
                </span>
                <Show when=move || !value.get().is_empty()>
                    <button
                        type="button"
                        aria-label=format!("Effacer {label}")
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
                {field("Du", from, Focus::From)} {field("Au", to, Focus::To)}
            </div>
            <Show when=move || calendar_open.get()>
                <div class="flex flex-col gap-3">
                    <div class="mb-1 flex items-center justify-between gap-1">
                        <NavBtn direction="prev" disabled=Signal::derive(at_start) on_click=go_prev />
                        <div class="flex items-center gap-1">
                            <DropdownSelect
                                value=month_value
                                on_change=Callback::new(on_pick_month)
                                options=month_options
                                aria_label="Mois"
                            />
                            <DropdownSelect
                                value=year_value
                                on_change=Callback::new(on_pick_year)
                                options=year_options
                                aria_label="Année"
                            />
                        </div>
                        <NavBtn direction="next" disabled=Signal::derive(at_end) on_click=go_next />
                    </div>
                    <div>
                        {move || {
                            let (y, m) = view_month.get();
                            month_view(y, m, from_date, to_date, select_day)
                        }}
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// Rend une grille mensuelle (en-têtes + jours), modifiers range appliqués.
fn month_view(
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
                    {WEEKDAYS_FR
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
        if is_start {
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

    #[test]
    fn weekday_known_dates() {
        // 2026-06-05 est un vendredi -> index 4 (lundi=0).
        assert_eq!(
            weekday_monday0(Civil {
                y: 2026,
                m: 6,
                d: 5
            }),
            4
        );
        // 2000-01-01 est un samedi -> index 5.
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
        let g = month_grid(2026, 6); // juin 2026 : le 1er est un lundi
        assert_eq!(g.len(), 42);
        // 2026-06-01 est un lundi -> pas de jour outside en tête.
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
        // mars 2026 : le 1er est un dimanche -> 6 jours outside en tête.
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
