//! Bloc « Cité par » de la colonne de contenu : ce qui cite un document est du
//! contenu de ce document, pas un rebond de marge. Section ouverte, décisions
//! au-dessus des textes, et aucun compteur : la profondeur passe par la recherche
//! facettée. Partagé par la page article et la vue-lecture d'une division, qui
//! citent l'une comme l'autre décisions et textes.
//!
//! Grammaire du registre « contenu » : lignes pleine largeur séparées d'un
//! filet (titre, première phrase du résumé, badge de portée). La carte bordée
//! reste celle des rebonds de la colonne droite.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{CitingDecisionHit, LinkedTextRef, SectionCitingText};

use super::data::CitingResult;
use crate::helpers::decisions_href;

/// Intertitre de sous-bloc : petites capitales trackées du registre discret.
const SUBTITLE: &str = "text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]";
/// Lien de profondeur / de repli, sous une liste.
const DEEP_LINK: &str =
    "w-fit text-sm text-[var(--color-accent)] underline-offset-4 hover:underline";
/// Filet séparateur des lignes du registre « contenu ».
const ROW: &str = "border-t border-[var(--color-rule)] py-3 first:border-t-0 first:pt-0";
/// Précision d'une ligne : la relation qu'elle porte, sous le titre.
const DETAIL: &str = "text-xs text-[var(--color-ink-subtle)]";

/// Ce qui varie d'un document cité à l'autre : son nom dans les phrases du
/// bloc, sa facette `legalArticle` et le repli plein-texte de l'état vide. Le
/// rendu, lui, ne varie pas — un article et une division partagent lignes,
/// filet et intertitres.
#[derive(Clone)]
pub(crate) struct CitedBySubject {
    /// « cet article », « cette division ».
    pub name: &'static str,
    /// Facette `legalArticle` : toutes les décisions citant le document.
    pub all_href: String,
    /// Recherche plein-texte, servie quand le graphe n'a rien identifié.
    pub search_href: String,
}

/// Une ligne du sous-bloc « Textes », d'où qu'elle vienne : renvoi entrant du
/// graphe sur la page article, texte citant agrégé sur la page d'une division.
/// `detail` porte ce que la seconde a de plus — le nombre d'articles visés, qui
/// est le critère de son classement.
#[derive(Clone)]
pub(crate) struct CitedText {
    pub label: String,
    pub href: Option<String>,
    pub detail: Option<String>,
}

impl From<LinkedTextRef> for CitedText {
    fn from(r: LinkedTextRef) -> Self {
        Self {
            label: r.label,
            href: r.href,
            detail: None,
        }
    }
}

impl From<SectionCitingText> for CitedText {
    fn from(t: SectionCitingText) -> Self {
        let detail = match t.article_count {
            1 => "cite 1 article de la division".to_string(),
            n => format!("cite {n} articles de la division"),
        };
        Self {
            label: t.label,
            href: t.href,
            detail: Some(detail),
        }
    }
}

/// Section « Cité par » de la page article : décisions citantes (streamées) puis
/// textes citants (portés par la réponse bloquante). `search_query` alimente le
/// repli vers la recherche plein-texte quand le graphe n'a rien identifié.
#[component]
pub(crate) fn CitedBySection(
    texts: Vec<LinkedTextRef>,
    citing: Resource<CitingResult>,
    search_query: String,
    code: String,
    num_key: String,
) -> impl IntoView {
    let subject = CitedBySubject {
        name: "cet article",
        all_href: decisions_href([("legalArticle", format!("{code}|{num_key}"))]),
        search_href: decisions_href([("q", search_query)]),
    };
    let texts: Vec<CitedText> = texts.into_iter().map(CitedText::from).collect();
    view! {
        <Suspense fallback=move || {
            view! {
                <p class="text-sm text-[var(--color-ink-subtle)]">"Chargement des décisions…"</p>
            }
        }>
            {move || {
                let texts = texts.clone();
                let subject = subject.clone();
                Suspend::new(async move { section(citing.await, texts, &subject, &[]) })
            }}
        </Suspense>
    }
}

/// « Cité par » : décisions au-dessus des textes. Le sous-bloc décisions sort
/// toujours — c'est justement quand le graphe n'a rien identifié que le repli
/// plein-texte sert. Les textes, eux, n'ont pas d'état vide.
///
/// `articles` = les articles du document en ordre de lecture (clé canonique,
/// numéro affichable) : sur une division, chaque ligne de décision dit par quel
/// article elle entre. Vide sur un article, où la réponse est dans l'URL.
pub(crate) fn section(
    resolved: CitingResult,
    texts: Vec<CitedText>,
    subject: &CitedBySubject,
    articles: &[(String, String)],
) -> AnyView {
    let CitingResult { hits, error } = resolved;
    let mut blocks = vec![decisions_block(hits, error, subject, articles)];
    if !texts.is_empty() {
        blocks.push(texts_block(texts));
    }
    frame(blocks)
}

/// Cadre commun de la section : le titre, puis ses sous-blocs dans l'ordre reçu.
fn frame(blocks: Vec<AnyView>) -> AnyView {
    view! {
        <section aria-label="Cité par" class="flex flex-col gap-6">
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Cité par"</h2>
            {blocks}
        </section>
    }
    .into_any()
}

/// Sous-bloc décisions : les cinq hits, puis le lien vers la facette
/// `legalArticle` (query montée par `ParamsMap` pour que le `|` soit encodé).
fn decisions_block(
    hits: Vec<CitingDecisionHit>,
    error: Option<String>,
    subject: &CitedBySubject,
    articles: &[(String, String)],
) -> AnyView {
    if let Some(err) = error {
        return view! {
            <div class="flex flex-col gap-2">
                <p class=SUBTITLE>"Décisions"</p>
                <p class="text-sm text-[var(--color-ink-subtle)]">
                    {format!("Décisions citantes indisponibles ({err}).")}
                </p>
            </div>
        }
        .into_any();
    }
    if hits.is_empty() {
        // Les backlinks `legal_citation` n'ont rien identifié : repli sur la
        // recherche plein-texte, qui attrape les mentions non extraites.
        return view! {
            <div class="flex flex-col gap-2">
                <p class=SUBTITLE>"Décisions"</p>
                <p class="text-sm text-[var(--color-ink-subtle)]">
                    {format!("Aucune décision citant {} n'a encore été identifiée.", subject.name)}
                </p>
                <A href=subject.search_href.clone() attr:class=DEEP_LINK>
                    "Rechercher dans les décisions"
                </A>
            </div>
        }
        .into_any();
    }
    let rows = hits
        .into_iter()
        .map(|hit| {
            let cited = cited_articles_line(&hit.cited_articles, articles);
            view! { <CitedDecisionRow hit=hit cited=cited /> }
        })
        .collect_view();
    view! {
        <div class="flex flex-col gap-3">
            <p class=SUBTITLE>"Décisions"</p>
            <ul class="flex flex-col">{rows}</ul>
            <A href=subject.all_href.clone() attr:class=DEEP_LINK>
                {format!("Voir toutes les décisions citant {}", subject.name)}
            </A>
        </div>
    }
    .into_any()
}

/// Nombre d'articles nommés sur la ligne d'une décision citant une division
/// avant de passer au décompte.
const NAMED_ARTICLES: usize = 3;

/// Par quel article la décision entre dans la division : « cite l'art. 748-6 »,
/// « cite les art. 748-1, 748-6 et 748-7 », « cite les art. 700, 750, 751 et
/// 9 autres ». Rien quand la question portait sur un article — la réponse est
/// dans l'URL — ni quand aucun des articles cités n'est nommable.
///
/// Les articles sortent dans l'ordre de lecture du document (celui de
/// `articles`), pas dans celui du tableau reçu, et sous leur numéro affichable.
fn cited_articles_line(cited: &[String], articles: &[(String, String)]) -> Option<String> {
    if cited.is_empty() {
        return None;
    }
    let labels: Vec<&str> = articles
        .iter()
        .filter(|(num_key, _)| cited.iter().any(|k| k == num_key))
        .map(|(_, label)| label.as_str())
        .collect();
    let (named, rest) = labels.split_at(labels.len().min(NAMED_ARTICLES));
    match (named, rest.len()) {
        ([], _) => None,
        ([one], 0) => Some(format!("cite l'art. {one}")),
        (named, 0) => {
            let (last, head) = named.split_last()?;
            Some(format!("cite les art. {} et {last}", head.join(", ")))
        }
        (named, more) => Some(format!(
            "cite les art. {} et {more} autres",
            named.join(", ")
        )),
    }
}

/// Une décision citante : titre lié, première phrase du résumé dépliable,
/// articles de la division qu'elle cite, badge de portée.
///
/// Ligne rendue par un **composant** : les enfants du `collect_view()` restent
/// uniformes et les nœuds optionnels vivent derrière la frontière du composant.
/// Neutraliser un nœud par `class="hidden"` avec un texte vide ne marche PAS :
/// le SSR n'émet alors aucun nœud texte, et l'hydratation qui en cherche un
/// panique en release (`failed_to_cast_text_node`).
#[component]
pub(crate) fn CitedDecisionRow(hit: CitingDecisionHit, cited: Option<String>) -> impl IntoView {
    use crate::components::ui::ExpandableSummary;
    let href = lj_dtos::decision_path(
        hit.jurisdiction_type,
        hit.jurisdiction_code.as_deref(),
        &hit.id,
    );
    let badge = crate::pages::decision_page::labels::publication_badge(&hit.publication_codes)
        .map(|label| {
            view! {
                <span class="inline-flex w-fit items-center rounded-full border border-[var(--color-rule)] px-2 py-0.5 text-[11px] text-[var(--color-ink-muted)]">
                    {label}
                </span>
            }
            .into_any()
        });
    let summary = hit.summary.map(|full| {
        let excerpt = crate::seo::decision::first_sentence(&full);
        view! { <ExpandableSummary excerpt=excerpt full=full /> }.into_any()
    });
    let cited = cited.map(|line| view! { <p class=DETAIL>{line}</p> }.into_any());
    view! {
        <li class=format!("flex flex-col gap-1.5 {ROW}")>
            <h3 class="font-sans text-sm leading-snug text-[var(--color-ink)]">
                <A
                    href=href
                    attr:class="no-underline transition-colors hover:text-[var(--color-accent)]"
                >
                    {hit.title}
                </A>
            </h3>
            {summary}
            {cited}
            {badge}
        </li>
    }
}

/// Sous-bloc textes : renvois entrants du graphe sur un article (libellé
/// verbatim DILA), textes citants agrégés sur une division.
fn texts_block(texts: Vec<CitedText>) -> AnyView {
    let rows = texts
        .into_iter()
        .map(|text| view! { <CitedTextRow text=text /> })
        .collect_view();
    view! {
        <div class="flex flex-col gap-3">
            <p class=SUBTITLE>"Textes"</p>
            <ul class="flex flex-col">{rows}</ul>
        </div>
    }
    .into_any()
}

/// Un texte citant : lié quand la cible est en base, texte nu sinon. Composant
/// pour la même raison que [`CitedDecisionRow`].
#[component]
fn CitedTextRow(text: CitedText) -> impl IntoView {
    // Même grammaire de titre que la ligne de décision (encre, accent au
    // survol) : les deux relations du bloc se citent pareil.
    let inner = match text.href {
        Some(href) => view! {
            <A
                href=href
                attr:class="no-underline transition-colors hover:text-[var(--color-accent)]"
            >
                {text.label}
            </A>
        }
        .into_any(),
        None => view! { <span>{text.label}</span> }.into_any(),
    };
    let detail = text
        .detail
        .map(|d| view! { <p class=DETAIL>{d}</p> }.into_any());
    view! {
        <li class=format!("flex flex-col gap-1.5 {ROW}")>
            <h3 class="font-sans text-sm leading-snug text-[var(--color-ink)]">{inner}</h3>
            {detail}
        </li>
    }
}
