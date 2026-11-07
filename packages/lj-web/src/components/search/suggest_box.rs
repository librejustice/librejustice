//! Autocomplétion des barres de recherche : contrôleur + panneau.
//!
//! Le parent garde son `<input>` contrôlé ; il crée un [`SuggestController`] sur
//! le même signal `query`, lui attache `on:keydown` / `on:blur`, pose `relative`
//! sur le conteneur du champ et rend un [`SuggestPanel`] dedans. Le contrôleur
//! observe `query` (debounce 150 ms), interroge `GET /suggest` et remplace les
//! `matched_tokens` derniers mots à la sélection. Gestes : les **flèches**
//! recopient la suggestion surlignée dans le champ (liste figée, remonter au-delà
//! restaure le texte tapé), donc **Enter** soumet le champ tel quel et la frappe
//! continue naturellement après ; le **clic** valide la suggestion et lance la
//! recherche aussitôt (soumission du formulaire englobant).

use leptos::leptos_dom::helpers::{set_timeout_with_handle, TimeoutHandle};
use leptos::prelude::*;
use leptos::task::spawn_local;

use lj_dtos::UiAction;

use crate::api::ApiClient;

/// Contrat de `/suggest` : min 2 codepoints saisis.
const MIN_QUERY: usize = 2;
/// Debounce de frappe avant fetch.
const DEBOUNCE_MS: u64 = 150;

#[derive(Clone, Copy)]
pub struct SuggestController {
    query: RwSignal<String>,
    mode: Signal<&'static str>,
    items: RwSignal<Vec<String>>,
    matched: RwSignal<u32>,
    active: RwSignal<Option<usize>>,
    open: RwSignal<bool>,
    debounce: StoredValue<Option<TimeoutHandle>>,
    /// Requête qui a produit `items`/`matched` : base du préfixe conservé et
    /// texte restauré quand la navigation aux flèches revient au champ.
    typed: StoredValue<String>,
    /// La mutation de `query` en cours est programmatique (flèches, clic) :
    /// pas de refetch pour celle-là — aux flèches la liste reste figée, au
    /// clic la recherche part.
    silent_set: StoredValue<bool>,
    /// Le champ a le focus : seule la frappe déclenche des suggestions.
    focused: RwSignal<bool>,
}

impl SuggestController {
    /// Branche le contrôleur sur le signal du champ. `mode` suit la route
    /// (`jurisprudence` | `textes` | `annuaire`).
    pub fn new(query: RwSignal<String>, mode: Signal<&'static str>) -> Self {
        let ctrl = Self {
            query,
            mode,
            items: RwSignal::new(Vec::new()),
            matched: RwSignal::new(0),
            active: RwSignal::new(None),
            open: RwSignal::new(false),
            debounce: StoredValue::new(None),
            typed: StoredValue::new(String::new()),
            silent_set: StoredValue::new(false),
            focused: RwSignal::new(false),
        };
        // Client-only (Effect ne tourne pas en SSR) : chaque frappe replanifie
        // le fetch.
        Effect::new(move |_| {
            let q = ctrl.query.get();
            if let Some(handle) = ctrl.debounce.get_value() {
                handle.clear();
            }
            if ctrl.silent_set.get_value() {
                ctrl.silent_set.set_value(false);
                return;
            }
            // Corpus sans index de suggestions : rien à demander.
            if ctrl.mode.get_untracked().is_empty()
                || !ctrl.focused.get_untracked()
                || q.trim().chars().count() < MIN_QUERY
            {
                ctrl.close();
                return;
            }
            let handle = set_timeout_with_handle(
                move || ctrl.fetch(q.clone()),
                std::time::Duration::from_millis(DEBOUNCE_MS),
            )
            .ok();
            ctrl.debounce.set_value(handle);
        });
        ctrl
    }

    fn fetch(self, q: String) {
        let mode = self.mode.get_untracked();
        spawn_local(async move {
            let Ok(resp) = ApiClient::from_context().suggest(&q, mode).await else {
                return;
            };
            // Réponse périmée (le champ a bougé pendant le fetch) : ignorer.
            if self.query.get_untracked() != q {
                return;
            }
            self.typed.set_value(q);
            self.matched.set(resp.matched_tokens);
            self.active.set(None);
            self.open.set(!resp.suggestions.is_empty());
            self.items.set(resp.suggestions);
        });
    }

    /// Requête tapée coupée en deux : les mots que la sélection conserve et
    /// ceux que la suggestion remplace (les `matched` derniers). Base de
    /// composition de [`Self::composed`] et du rendu du panneau.
    fn typed_split(self) -> (String, String) {
        let matched = self.matched.get() as usize;
        self.typed.with_value(|q| {
            let words: Vec<&str> = q.split_whitespace().collect();
            let cut = words.len().saturating_sub(matched);
            (words[..cut].join(" "), words[cut..].join(" "))
        })
    }

    /// Mots de la requête tapée que la sélection conserve devant la suggestion.
    fn kept_prefix(self) -> String {
        self.typed_split().0
    }

    /// Requête résultante si on retient la suggestion `i`.
    fn composed(self, i: usize) -> Option<String> {
        let suggestion = self.items.with_untracked(|it| it.get(i).cloned())?;
        let kept = self.kept_prefix();
        Some(if kept.is_empty() {
            suggestion
        } else {
            format!("{kept} {suggestion}")
        })
    }

    /// Valide la suggestion `i` (clic) : elle remplace les `matched` derniers mots
    /// tapés et la recherche part aussitôt — soumission du formulaire qui contient le
    /// champ.
    pub fn pick(self, i: usize, ev: &leptos::ev::MouseEvent) {
        let Some(next) = self.composed(i) else {
            return;
        };
        self.silent_set.set_value(true);
        self.query.set(next);
        self.close();
        submit_enclosing_form(ev);
    }

    /// Navigation aux flèches : la suggestion surlignée est **recopiée dans
    /// le champ** (`None` = retour au texte tapé), la liste reste figée.
    fn navigate(self, next: Option<usize>) {
        let text = match next {
            Some(i) => {
                let Some(t) = self.composed(i) else { return };
                t
            }
            None => self.typed.get_value(),
        };
        self.active.set(next);
        self.silent_set.set_value(true);
        self.query.set(text);
    }

    pub fn close(self) {
        self.open.set(false);
        self.active.set(None);
        self.items.set(Vec::new());
    }

    /// Set programmatique de `query` par le parent (resync depuis l'URL) : pas de
    /// refetch, le panneau ne se rouvre pas après une soumission.
    pub fn sync(self, text: String) {
        self.silent_set.set_value(true);
        self.query.set(text);
    }

    /// Navigation clavier à attacher à l'`<input>` : flèches (appliquent la
    /// suggestion dans le champ, cycle retour au texte tapé), Escape. Enter
    /// n'est pas intercepté : le champ contient déjà ce qu'il faut, la
    /// soumission du formulaire suit son cours (on ferme juste le panneau).
    pub fn on_keydown(self) -> impl Fn(leptos::ev::KeyboardEvent) + Clone {
        move |ev: leptos::ev::KeyboardEvent| {
            // L'`autofocus` (landing) prend le focus avant l'hydration : le
            // `focus` natif n'émet alors aucun événement capté — la frappe
            // vaut preuve de focus.
            self.focused.set(true);
            if !self.open.get_untracked() {
                return;
            }
            let len = self.items.with_untracked(Vec::len);
            match ev.key().as_str() {
                "ArrowDown" => {
                    ev.prevent_default();
                    self.navigate(match self.active.get_untracked() {
                        None => Some(0),
                        Some(i) if i + 1 == len => None,
                        Some(i) => Some(i + 1),
                    });
                }
                "ArrowUp" => {
                    ev.prevent_default();
                    self.navigate(match self.active.get_untracked() {
                        None => Some(len - 1),
                        Some(0) => None,
                        Some(i) => Some(i - 1),
                    });
                }
                "Enter" => self.close(),
                "Escape" => self.close(),
                _ => {}
            }
        }
    }

    /// À attacher au focus du champ (arme le déclenchement par frappe).
    pub fn on_focus(self) -> impl Fn(leptos::ev::FocusEvent) + Clone {
        move |_| self.focused.set(true)
    }

    /// Fermeture au blur du champ (le `mousedown` du panneau garde le focus,
    /// donc une sélection à la souris ne passe jamais par ici).
    pub fn on_blur(self) -> impl Fn(leptos::ev::FocusEvent) + Clone {
        move |_| {
            self.focused.set(false);
            self.close();
        }
    }
}

/// Panneau déroulant, à rendre dans le conteneur `relative` du champ.
#[component]
pub fn SuggestPanel(ctrl: SuggestController) -> impl IntoView {
    view! {
            <ul
                role="listbox"
                aria-label="Suggestions"
                data-open=move || ctrl.open.get().then_some("true")
                class="lj-panel absolute inset-x-0 top-full z-30 mt-1 overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] py-1 shadow-lg"
            >
                <For
                    each=move || ctrl.items.get().into_iter().enumerate()
                    key=|(i, s)| (*i, s.clone())
                    children=move |(i, suggestion)| {
                        view! {
                            <li>
                                <button
                                    type="button"
                                    role="option"
                                    data-usage=UiAction::SearchSuggestion.as_str()
                                    aria-selected=move || (ctrl.active.get() == Some(i)).to_string()
                                    // mousedown : préempte le blur de l'input (pas de
                                    // course fermeture/clic), le click fait le pick.
                                    on:mousedown=move |ev| ev.prevent_default()
                                    on:click=move |ev| ctrl.pick(i, &ev)
                                    on:mouseenter=move |_| ctrl.active.set(Some(i))
                                    class=move || {
                                        let active = ctrl.active.get() == Some(i);
                                        format!(
                                            "w-full cursor-pointer px-3 py-1.5 text-left text-sm transition-colors {}",
                                            if active {
                                                "bg-[var(--color-vellum)] text-[var(--color-ink)]"
                                            } else {
                                                "text-[var(--color-ink-muted)]"
                                            },
                                        )
                                    }
                                >
                                    {move || {
                                        let (kept, replaced) = ctrl.typed_split();
                                        let (typed, added) = split_added(
                                            &kept,
                                            &replaced,
                                            &suggestion,
                                        );
                                        view! {
                                            <span class="opacity-50">{typed}</span>
                                            <span class="font-medium">{added}</span>
                                        }
                                    }}
                                </button>
                            </li>
                        }
                    }
                />
            </ul>
    }
}

/// Ligne du panneau coupée en (déjà tapé, apport de la suggestion) : `kept` est
/// le début de requête conservé, `replaced` la fin que la suggestion remplace et
/// dont elle reprend le plus souvent les premiers caractères. Seul l'apport est
/// mis en valeur ; ce que l'utilisateur a déjà écrit reste estompé, même quand
/// la suggestion le réécrit avec ses accents. Aucun recouvrement (une
/// dénomination de l'annuaire, qui remplace la requête entière) ⇒ tout est
/// apport.
fn split_added(kept: &str, replaced: &str, suggestion: &str) -> (String, String) {
    let typed: Vec<char> = replaced.chars().flat_map(lj_fold::fold_char).collect();
    let mut cut = 0;
    let mut at = 0;
    for c in suggestion.chars() {
        if at == typed.len() {
            break;
        }
        let folded: Vec<char> = lj_fold::fold_char(c).collect();
        let rest = &typed[at..];
        if rest.starts_with(&folded) {
            at += folded.len();
        } else if folded.starts_with(rest) {
            at = typed.len();
        } else {
            break;
        }
        cut += c.len_utf8();
    }
    // L'espace qui suit appartient au déjà-tapé : la frappe s'arrêtait dessus.
    cut += suggestion[cut..].len() - suggestion[cut..].trim_start().len();
    let mut typed = kept.to_string();
    if !typed.is_empty() {
        typed.push(' ');
    }
    typed.push_str(&suggestion[..cut]);
    (typed, suggestion[cut..].to_string())
}

/// Soumet le formulaire qui contient la suggestion cliquée (son `on:submit`
/// navigue vers la recherche).
#[cfg(feature = "hydrate")]
fn submit_enclosing_form(ev: &leptos::ev::MouseEvent) {
    use wasm_bindgen::JsCast;
    let form = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|el| el.closest("form").ok().flatten())
        .and_then(|el| el.dyn_into::<web_sys::HtmlFormElement>().ok());
    if let Some(form) = form {
        let _ = form.request_submit();
    }
}

#[cfg(feature = "ssr")]
fn submit_enclosing_form(_ev: &leptos::ev::MouseEvent) {}

#[cfg(test)]
mod tests {
    use super::split_added;

    #[test]
    fn seul_l_apport_de_la_suggestion_est_mis_en_valeur() {
        let (typed, added) = split_added(
            "responsabilité médicale",
            "infection",
            "infection nosocomiale",
        );
        assert_eq!(typed, "responsabilité médicale infection ");
        assert_eq!(added, "nosocomiale");
    }

    #[test]
    fn mot_en_cours_de_frappe_coupe_dans_le_mot() {
        let (typed, added) = split_added(
            "responsabilité médicale",
            "infection n",
            "infection nosocomiale",
        );
        assert_eq!(typed, "responsabilité médicale infection n");
        assert_eq!(added, "osocomiale");
    }

    #[test]
    fn frappe_sans_accent_rendue_avec_ceux_de_la_suggestion() {
        let (typed, added) = split_added("", "conges pa", "congés payés");
        assert_eq!(typed, "congés pa");
        assert_eq!(added, "yés");
    }

    #[test]
    fn suggestion_sans_recouvrement_entierement_en_apport() {
        let (typed, added) = split_added("", "dupont", "SELARL Martin");
        assert_eq!(typed, "");
        assert_eq!(added, "SELARL Martin");
    }
}
