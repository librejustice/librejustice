//! Briques partagées des filtres décisions : navigation (`Nav`), clés de filtre,
//! arbre à facette, lignes checkbox (`CheckboxOption`, `TreeRow`), recherche
//! intra-facette et checklist plate plafonnée (`FacetChecklist`).
//!
//! Les options viennent des facettes servies (`FacetChoice` : valeur + libellé +
//! `parent`) — aucune table de libellés compilée. Une valeur sélectionnée absente
//! de la facette (URL partagée, compteur nul) reste affichée en ligne orpheline
//! (libellé = valeur brute, compteur 0).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;
use lj_dtos::FacetChoice;

use crate::helpers::{cn, group_thousands};

use super::compact_search::query_state;

/// Clés de filtre décisions.
pub fn filter_keys() -> Vec<&'static str> {
    vec![
        "seat",
        "formation",
        // Keys no longer written here, so that an old bookmark stays clearable.
        "jurisdictionType",
        "jurisdictionCode",
        "office",
        "procedure",
        "legalDomain",
        "solution",
        "publication",
        "legalInstrument",
        "legalArticle",
        "dateFrom",
        "dateTo",
        "searchIn",
    ]
}

/// Pause before a facet search applies, so that a keystroke on the way to a
/// word does not render the tree it names in passing.
const FILTER_DEBOUNCE: Duration = Duration::from_millis(180);

/// A facet choice and its descendants, at any depth.
#[derive(Clone, Debug, PartialEq)]
pub struct FacetNode {
    pub choice: FacetChoice,
    pub children: Vec<FacetNode>,
}

/// Groups a flat facet into a tree of free depth: roots (`parent = None`), then
/// attachment by `parent` == `value`, verbatim. A node hanging from a parent the
/// facet does not serve is raised to the root instead of being dropped.
pub fn build_tree(choices: &[FacetChoice]) -> Vec<FacetNode> {
    let mut children_of: HashMap<&str, Vec<&FacetChoice>> = HashMap::new();
    for c in choices {
        if let Some(parent) = c.parent.as_deref() {
            children_of.entry(parent).or_default().push(c);
        }
    }
    let mut grown: HashSet<&str> = HashSet::new();
    let mut roots: Vec<FacetNode> = Vec::new();
    for c in choices.iter().filter(|c| c.parent.is_none()) {
        roots.extend(grow(c, &children_of, &mut grown));
    }
    for c in choices {
        roots.extend(grow(c, &children_of, &mut grown));
    }
    roots
}

/// `grown` holds what is already rendered: it closes the recursion on a
/// circular parentage.
fn grow<'a>(
    choice: &'a FacetChoice,
    children_of: &HashMap<&'a str, Vec<&'a FacetChoice>>,
    grown: &mut HashSet<&'a str>,
) -> Option<FacetNode> {
    if !grown.insert(&choice.value) {
        return None;
    }
    let children = children_of
        .get(choice.value.as_str())
        .map(|kids| {
            kids.iter()
                .filter_map(|k| grow(k, children_of, grown))
                .collect()
        })
        .unwrap_or_default();
    Some(FacetNode {
        choice: choice.clone(),
        children,
    })
}

/// Reduces the tree to what answers `needle` (already lowercased), at any
/// depth: a branch that matches keeps all its descendants, otherwise it keeps
/// only the descendants that match. Also returns the nodes the search must
/// UNFOLD, which are only those a match sits under: « paris » names several courts,
/// and a room is reached by opening its court rather than by unfolding them all. A
/// branch that answers by its own label stays folded.
pub fn prune_tree(tree: Vec<FacetNode>, needle: &str) -> (Vec<FacetNode>, HashSet<String>) {
    if needle.is_empty() {
        return (tree, HashSet::new());
    }
    let mut unfold = HashSet::new();
    let kept = tree
        .into_iter()
        .filter_map(|node| prune_node(node, needle, &mut unfold))
        .collect();
    (kept, unfold)
}

fn prune_node(node: FacetNode, needle: &str, unfold: &mut HashSet<String>) -> Option<FacetNode> {
    if node.choice.label.to_lowercase().contains(needle) {
        return Some(node);
    }
    let children: Vec<FacetNode> = node
        .children
        .into_iter()
        .filter_map(|c| prune_node(c, needle, unfold))
        .collect();
    if children.is_empty() {
        return None;
    }
    unfold.insert(node.choice.value.clone());
    Some(FacetNode {
        choice: node.choice,
        children,
    })
}

type NavFn = Box<dyn Fn(&str, NavigateOptions) + Send + Sync>;

/// Navigateur partagé : applique une query string et navigue (replace=true).
///
/// `navigate` (Send+Sync) est stocké en `StoredValue` (SyncStorage) : le handle
/// est Copy, donc `Nav` est Copy et peut être capturé par les `Callback::new`
/// (qui exigent Send+Sync) des sous-composants. `new_local` (LocalStorage)
/// attacherait la valeur à l'owner courant, introuvable depuis le contexte
/// détaché d'un `Callback` ; SyncStorage la rend lisible partout.
#[derive(Clone, Copy)]
pub struct Nav {
    navigate: StoredValue<NavFn>,
}

impl Nav {
    /// À appeler dans un contexte de composant (résout `use_navigate`).
    pub fn new() -> Self {
        Self {
            navigate: StoredValue::new(Box::new(use_navigate()) as NavFn),
        }
    }

    pub fn go(&self, qs: String) {
        self.navigate.with_value(|n| {
            n(
                &query_state::search_href(&qs),
                NavigateOptions {
                    replace: true,
                    // `scroll` défaut `true` : le router remonterait en haut à
                    // chaque mutation de facette. Les filtres affinent la recherche
                    // courante (replace=true), ils ne naviguent pas.
                    scroll: false,
                    ..Default::default()
                },
            )
        });
    }
}

impl Default for Nav {
    fn default() -> Self {
        Self::new()
    }
}

// ── CheckboxOption ────────────────────────────────────────────────────────────

/// Vrai quand les nombres en main comptent un autre ensemble que les filtres de
/// l'URL : la ligne reste cliquable, son compteur se tait. Hors provider, les
/// compteurs s'affichent.
#[derive(Clone, Copy)]
pub struct StaleCounts(pub Signal<bool>);

/// À lire une fois par bloc qui rend des compteurs.
pub fn stale_counts() -> Signal<bool> {
    use_context::<StaleCounts>()
        .map(|s| s.0)
        .unwrap_or_else(|| Signal::derive(|| false))
}

#[component]
pub fn CheckboxOption(
    // `label` réactif : pour « Textes cités » le libellé (titre catalogue) arrive
    // avec les facettes, après la ligne (token d'URL). Les autres call-sites
    // passent un `String` (converti via `into`).
    #[prop(into)] label: Signal<String>,
    // `count` réactif : sur arrivée des facettes (nouvelle recherche) les compteurs
    // changent en place sans reconstruire la ligne.
    #[prop(into)] count: Signal<i64>,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] on_toggle: Callback<()>,
    #[prop(optional)] indent: bool,
    // Ce que la valeur recouvre quand elle en regroupe plusieurs, servi par le
    // référentiel. Réactif : il arrive avec les facettes, après la ligne.
    #[prop(optional, into)] hint: Signal<Option<String>>,
) -> impl IntoView {
    let button_class = move || {
        cn([
            "flex w-full items-center gap-2.5 rounded px-2 py-1 text-left text-sm transition-colors",
            if indent { "pl-7" } else { "" },
            if selected.get() {
                "bg-[var(--color-bordeaux-soft)] text-[var(--color-accent)]"
            } else {
                "text-[var(--color-ink-muted)] hover:text-[var(--color-ink)]"
            },
        ])
    };
    let stale = stale_counts();
    let shows_count = move || count.get() > 0 && !stale.get();
    let box_class = move || {
        cn([
            "flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border transition-colors",
            if selected.get() {
                "border-[var(--color-accent)] bg-[var(--color-accent)]"
            } else {
                "border-[var(--color-rule)] bg-transparent"
            },
        ])
    };
    view! {
        <button
            type="button"
            on:click=move |_| on_toggle.run(())
            class=button_class
            title=move || hint.get()
        >
            <span class=box_class>
                <Show when=move || selected.get()>
                    <svg
                        viewBox="0 0 12 10"
                        class="h-2.5 w-3 text-white"
                        fill="none"
                        aria-hidden="true"
                    >
                        <path
                            d="M1 5l3.5 3.5L11 1"
                            stroke="currentColor"
                            stroke-width="1.8"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        />
                    </svg>
                </Show>
            </span>
            <span class="flex-1 leading-tight">{label}</span>
            <Show when=shows_count>
                <span class="tabular-nums text-xs text-[var(--color-ink-subtle)]">
                    {move || group_thousands(count.get())}
                </span>
            </Show>
        </button>
    }
}

// ── FilterSearchInput ─────────────────────────────────────────────────────────

#[component]
pub fn FilterSearchInput(
    value: RwSignal<String>,
    placeholder: &'static str,
    #[prop(optional)] on_enter: Option<Callback<()>>,
) -> impl IntoView {
    // The typed text is local and instant; `value`, which a facet tree hangs
    // off, follows after the pause. Typing « chambre sociale » crosses prefixes
    // that each keep most of the seat axis: without the pause the browser
    // renders every one of them.
    let typed = RwSignal::new(value.get_untracked());
    let pending = StoredValue::new_local(None::<TimeoutHandle>);
    let arm = move || {
        if let Some(handle) = pending.get_value() {
            handle.clear();
        }
        pending.set_value(
            set_timeout_with_handle(move || value.set(typed.get_untracked()), FILTER_DEBOUNCE).ok(),
        );
    };
    let on_input = move |ev: leptos::ev::Event| {
        typed.set(event_target_value(&ev));
        arm();
    };
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            if let Some(handle) = pending.get_value() {
                handle.clear();
                pending.set_value(None);
            }
            value.set(typed.get_untracked());
            if let Some(cb) = on_enter {
                ev.prevent_default();
                cb.run(());
            }
        }
    };
    view! {
        <div class="relative mb-1">
            <svg
                viewBox="0 0 16 16"
                class="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[var(--color-ink-subtle)]"
                fill="none"
                aria-hidden="true"
            >
                <circle cx="6.5" cy="6.5" r="4.5" stroke="currentColor" stroke-width="1.4" />
                <path d="M10.5 10.5l3 3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
            </svg>
            <input
                type="search"
                prop:value=move || typed.get()
                on:input=on_input
                on:keydown=on_keydown
                placeholder=placeholder
                class="w-full rounded border border-[var(--color-rule)] bg-transparent py-1 pl-7 pr-2 text-xs text-[var(--color-ink)] placeholder:text-[var(--color-ink-subtle)] focus:border-[var(--color-accent)] focus:outline-none"
            />
        </div>
    }
}

// ── TreeRow ───────────────────────────────────────────────────────────────────

/// Ligne dépliable : chevron + checkbox, corps monté au premier dépli. Sans
/// enfant le chevron garde sa place, invisible : les cases restent alignées.
#[component]
fn CollapsibleRow(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] count: Signal<i64>,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] has_children: Signal<bool>,
    /// Un descendant coché déplie la ligne, jamais l'inverse : le repli reste au
    /// chevron. `Memo` attendu, sinon l'effet se rejoue à chaque coche.
    #[prop(into)]
    auto_expand: Signal<bool>,
    #[prop(into)] on_toggle: Callback<()>,
    #[prop(optional, into)] hint: Signal<Option<String>>,
    children: ChildrenFn,
) -> impl IntoView {
    let expanded = RwSignal::new(auto_expand.get_untracked());
    Effect::new(move |_| {
        if auto_expand.get() {
            expanded.set(true);
        }
    });
    // Lazy-mount : plusieurs centaines de lignes sous une branche, dont les
    // compteurs recalculeraient à chaque arrivée de facettes.
    let has_opened = RwSignal::new(auto_expand.get_untracked());
    Effect::new(move |_| {
        if expanded.get() {
            has_opened.set(true);
        }
    });

    let chevron_class = move || {
        cn([
            "flex h-5 w-5 shrink-0 items-center justify-center rounded text-[var(--color-ink-subtle)] transition-colors hover:text-[var(--color-ink)]",
            if !has_children.get() { "pointer-events-none opacity-0" } else { "" },
        ])
    };

    view! {
        <div class="flex flex-col gap-1">
            <div class="flex items-center gap-1">
                <button
                    type="button"
                    aria-label=move || if expanded.get() { "Réduire" } else { "Développer" }
                    disabled=move || !has_children.get()
                    on:click=move |_| expanded.update(|v| *v = !*v)
                    class=chevron_class
                >
                    <svg
                        viewBox="0 0 12 12"
                        class=move || {
                            cn([
                                "h-3 w-3 transition-transform",
                                if expanded.get() { "rotate-90" } else { "" },
                            ])
                        }
                        fill="none"
                        aria-hidden="true"
                    >
                        <path
                            d="M4 2l4 4-4 4"
                            stroke="currentColor"
                            stroke-width="1.5"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        />
                    </svg>
                </button>
                <CheckboxOption
                    label=label
                    count=count
                    selected=selected
                    on_toggle=on_toggle
                    indent=false
                    hint=hint
                />
            </div>
            <Show when=move || has_children.get()>
                <div
                    class="grid"
                    style=move || {
                        format!(
                            "grid-template-rows: {}; transition: grid-template-rows 200ms ease-out",
                            if expanded.get() { "1fr" } else { "0fr" },
                        )
                    }
                >
                    <div class="min-h-0 overflow-hidden">
                        <div class="flex flex-col gap-1 pl-5 pt-0.5">
                            {
                                let children = children.clone();
                                move || has_opened.get().then(|| children())
                            }
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// One row of a facet tree and, under it, its own descendants: the body of the
/// recursion. The erased return type is what lets the view name itself.
#[component]
pub fn TreeRow(
    /// The row's siblings, reread on each arrival of the facets: the keyed row
    /// stays, its label and counter refresh in place.
    #[prop(into)]
    siblings: Signal<Vec<FacetNode>>,
    value: String,
    /// Selection of the whole axis, every depth mixed.
    #[prop(into)]
    selected: Signal<Vec<String>>,
    /// Nodes an intra-facet search must unfold, from `prune_tree`: a match sits
    /// under them, so the row opens without one more click. A row that answers
    /// by its own label is NOT in it and stays folded, its subtree unrendered.
    #[prop(optional, into)]
    unfold: Signal<HashSet<String>>,
    #[prop(into)] on_toggle: Callback<String>,
) -> AnyView {
    let lookup = value.clone();
    let row =
        Memo::new(move |_| siblings.with(|s| s.iter().find(|n| n.choice.value == lookup).cloned()));
    let fallback = value.clone();
    let label = Signal::derive(move || {
        row.get()
            .map(|n| n.choice.label)
            .unwrap_or(fallback.clone())
    });
    let count = Signal::derive(move || row.get().map(|n| n.choice.count).unwrap_or(0));
    let hint = Signal::derive(move || row.get().and_then(|n| n.choice.hint));
    let children = Memo::new(move |_| row.get().map(|n| n.children).unwrap_or_default());
    let has_children = Signal::derive(move || !children.get().is_empty());
    let sel_value = value.clone();
    let is_sel = Signal::derive(move || selected.get().contains(&sel_value));
    let unfold_value = value.clone();
    let has_selection = Memo::new(move |_| {
        unfold.with(|u| u.contains(&unfold_value))
            || children.with(|c| selected.with(|s| subtree_selected(c, s)))
    });
    view! {
        <CollapsibleRow
            label=label
            count=count
            selected=is_sel
            hint=hint
            has_children=has_children
            auto_expand=has_selection
            on_toggle=Callback::new(move |_| on_toggle.run(value.clone()))
        >
            <For
                each=move || children.get()
                key=|n: &FacetNode| n.choice.value.clone()
                children=move |n: FacetNode| {
                    view! {
                        <TreeRow
                            siblings=children
                            value=n.choice.value
                            selected=selected
                            unfold=unfold
                            on_toggle=on_toggle
                        />
                    }
                }
            />
        </CollapsibleRow>
    }
    .into_any()
}

fn subtree_selected(nodes: &[FacetNode], selected: &[String]) -> bool {
    nodes
        .iter()
        .any(|n| selected.contains(&n.choice.value) || subtree_selected(&n.children, selected))
}

// ── FacetTree ─────────────────────────────────────────────────────────────────

/// Arbre à cocher d'une facette hiérarchisée (Domaine du droit, Matière) : un
/// groupe et ses membres se filtrent par la même clé.
#[component]
pub fn FacetTree(
    #[prop(into)] choices: Signal<Vec<FacetChoice>>,
    #[prop(into)] selected: Signal<Vec<String>>,
    #[prop(into)] on_toggle: Callback<String>,
) -> impl IntoView {
    let tree = Memo::new(move |_| build_tree(&choices.get()));
    let orphans = Memo::new(move |_| {
        let choices = choices.get();
        selected
            .get()
            .into_iter()
            .filter(|s| !choices.iter().any(|c| &c.value == s))
            .collect::<Vec<_>>()
    });
    view! {
        <div class="filter-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto overscroll-contain">
            <For
                each=move || tree.get()
                key=|n: &FacetNode| n.choice.value.clone()
                children=move |n: FacetNode| {
                    view! {
                        <TreeRow
                            siblings=tree
                            value=n.choice.value
                            selected=selected
                            on_toggle=on_toggle
                        />
                    }
                }
            />
            <For
                each=move || orphans.get()
                key=|value: &String| value.clone()
                children=move |value: String| {
                    let sel_value = value.clone();
                    let is_sel = Signal::derive(move || selected.get().contains(&sel_value));
                    let toggle_value = value.clone();
                    view! {
                        <CheckboxOption
                            label=value.clone()
                            count=Signal::derive(|| 0)
                            selected=is_sel
                            on_toggle=Callback::new(move |_| on_toggle.run(toggle_value.clone()))
                            indent=false
                        />
                    }
                }
            />
        </div>
    }
}

// ── FacetChecklist ────────────────────────────────────────────────────────────

/// Liste plate à cocher pilotée par les facettes servies (Publication,
/// Dispositif — sections du modal « Plus de filtres »). Lignes = sélection
/// orpheline (libellé = valeur brute, compteur 0) ∪ facette. Plafonnée à 6 lignes
/// + « Voir plus ».
#[component]
pub fn FacetChecklist(
    #[prop(into)] choices: Signal<Vec<FacetChoice>>,
    #[prop(into)] selected: Signal<Vec<String>>,
    #[prop(into)] on_toggle: Callback<String>,
) -> impl IntoView {
    const CAP: usize = 6;
    let show_all = RwSignal::new(false);
    // `Memo` : lu par les signaux `count`/`label` de chaque ligne.
    let rows = Memo::new(move |_| {
        let choices = choices.get();
        let mut out: Vec<FacetChoice> = selected
            .get()
            .into_iter()
            .filter(|s| !choices.iter().any(|c| &c.value == s))
            .map(|s| FacetChoice {
                label: s.clone(),
                value: s,
                count: 0,
                parent: None,
                hint: None,
            })
            .collect();
        out.extend(choices);
        out
    });
    let overflow = Signal::derive(move || rows.with(|r| r.len()) > CAP);
    let visible = Memo::new(move |_| {
        let rows = rows.get();
        if show_all.get() {
            rows
        } else {
            rows.into_iter().take(CAP).collect()
        }
    });
    view! {
        <div class="flex flex-col gap-1.5">
            <For
                each=move || visible.get()
                key=|c: &FacetChoice| c.value.clone()
                children=move |c: FacetChoice| {
                    let value = c.value.clone();
                    let lookup = c.value.clone();
                    let row = Memo::new(move |_| {
                        rows.with(|l| l.iter().find(|x| x.value == lookup).cloned())
                    });
                    let fallback = c.value.clone();
                    let row_label = Signal::derive(move || {
                        row.get().map(|x| x.label).unwrap_or(fallback.clone())
                    });
                    let row_count =
                        Signal::derive(move || row.get().map(|x| x.count).unwrap_or(0));
                    let sel_value = c.value.clone();
                    let is_sel =
                        Signal::derive(move || selected.get().contains(&sel_value));
                    let row_hint = Signal::derive(move || row.get().and_then(|x| x.hint));
                    view! {
                        <CheckboxOption
                            label=row_label
                            count=row_count
                            selected=is_sel
                            on_toggle=Callback::new(move |_| on_toggle.run(value.clone()))
                            indent=false
                            hint=row_hint
                        />
                    }
                }
            />
            <Show when=move || overflow.get()>
                <button
                    type="button"
                    on:click=move |_| show_all.update(|v| *v = !*v)
                    class="self-start px-2 text-xs text-[var(--color-accent)] hover:underline"
                >
                    {move || if show_all.get() { "Voir moins" } else { "Voir plus" }}
                </button>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choix(value: &str, label: &str, parent: Option<&str>) -> FacetChoice {
        FacetChoice {
            value: value.to_string(),
            label: label.to_string(),
            count: 1,
            parent: parent.map(str::to_string),
            hint: None,
        }
    }

    /// Les formations d'une cour non repliée sont au troisième étage : sans
    /// élagage à cet étage, une saisie sur leur libellé ne rend rien.
    #[test]
    fn prune_tree_atteint_le_troisieme_etage() {
        let plat = vec![
            choix("CA", "Cour d'appel", None),
            choix("ca_versailles", "Cour d'appel de Versailles", Some("CA")),
            choix(
                "ca_versailles|C4-7",
                "Chambre 4-7, protection sociale",
                Some("ca_versailles"),
            ),
            choix("ca_versailles|C5", "5e chambre", Some("ca_versailles")),
            choix("CC", "Cour de cassation", None),
            choix("cc|SOCIALE", "Chambre sociale", Some("CC")),
        ];

        let (tree, _) = prune_tree(build_tree(&plat), "protection sociale");
        assert_eq!(tree.len(), 1, "seule la branche qui matche reste");
        assert_eq!(tree[0].choice.value, "CA");
        assert_eq!(tree[0].children.len(), 1);
        let cour = &tree[0].children[0];
        assert_eq!(cour.choice.value, "ca_versailles");
        assert_eq!(cour.children.len(), 1, "la 5e chambre ne matche pas");
        assert_eq!(cour.children[0].choice.value, "ca_versailles|C4-7");
    }

    /// A folded court carries its formations on the second level, and a branch
    /// whose own label answers keeps all its descendants without unfolding
    /// them: « paris » names five courts, and unfolding them showed their 203
    /// rooms.
    #[test]
    fn prune_tree_garde_la_descendance_d_une_branche_qui_matche() {
        let plat = vec![
            choix("CC", "Cour de cassation", None),
            choix("cc|SOCIALE", "Chambre sociale", Some("CC")),
            choix("cc|C2", "2e chambre civile", Some("CC")),
            choix("CA", "Cour d'appel", None),
            choix("ca_paris", "Cour d'appel de Paris", Some("CA")),
        ];

        let (sociale, ouvre) = prune_tree(build_tree(&plat), "sociale");
        assert_eq!(sociale[0].children.len(), 1);
        assert!(
            ouvre.contains("CC"),
            "le match est sous la Cour de cassation, elle s'ouvre"
        );

        let (cassation, ouvre) = prune_tree(build_tree(&plat), "cour de cassation");
        assert_eq!(cassation.len(), 1);
        assert_eq!(
            cassation[0].children.len(),
            2,
            "les deux formations restent"
        );
        assert!(
            ouvre.is_empty(),
            "la branche répond par son propre libellé : elle reste repliée"
        );

        assert_eq!(prune_tree(build_tree(&plat), "lyon").0.len(), 0);
        assert_eq!(prune_tree(build_tree(&plat), "").0.len(), 2, "sans saisie");
    }

    /// A court of appeal pole opens a fourth level.
    #[test]
    fn build_tree_descend_sous_le_troisieme_etage() {
        let plat = vec![
            choix("FR.CA", "Cour d'appel", Some("FR")),
            choix("FR.CA.PARIS", "Cour d'appel de Paris", Some("FR.CA")),
            choix("FR.CA.PARIS.P5", "Pôle 5, économique", Some("FR.CA.PARIS")),
            choix("FR.CA.PARIS.P5.C6", "6e chambre", Some("FR.CA.PARIS.P5")),
        ];

        let tree = build_tree(&plat);
        assert_eq!(tree.len(), 1);
        let pole = &tree[0].children[0].children[0];
        assert_eq!(pole.choice.value, "FR.CA.PARIS.P5");
        assert_eq!(pole.children[0].choice.value, "FR.CA.PARIS.P5.C6");

        let (trouve, ouvre) = prune_tree(tree, "6e chambre");
        assert_eq!(
            trouve[0].children[0].children[0].children[0].choice.value, "FR.CA.PARIS.P5.C6",
            "l'élagage garde la chaîne des ancêtres de la feuille"
        );
        assert_eq!(
            ouvre.len(),
            3,
            "les trois ancêtres s'ouvrent, la feuille n'a rien à ouvrir"
        );
    }

    /// A jurisdiction without a seat holds entirely at the first level.
    #[test]
    fn build_tree_rend_une_racine_sans_descendance() {
        let plat = vec![choix(
            "FR.CNDA",
            "Cour nationale du droit d'asile",
            Some("FR"),
        )];

        let tree = build_tree(&plat);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].choice.value, "FR.CNDA");
        assert!(tree[0].children.is_empty());
        assert_eq!(prune_tree(build_tree(&plat), "asile").0.len(), 1);
    }

    /// A parent the facet does not serve (API-side truncation): the branch is
    /// raised to the root, it does not vanish.
    #[test]
    fn build_tree_remonte_une_branche_sans_parent_servi() {
        let plat = vec![
            choix("FR.CA", "Cour d'appel", Some("FR")),
            choix("FR.CA.PARIS", "Cour d'appel de Paris", Some("FR.CA")),
            choix("FR.CA.LYON.C1", "1re chambre", Some("FR.CA.LYON")),
        ];

        let tree = build_tree(&plat);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[1].choice.value, "FR.CA.LYON.C1");
    }
}
