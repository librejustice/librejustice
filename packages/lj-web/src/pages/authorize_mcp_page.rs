//! Page `/authorize-mcp`.
//!
//! Écran de consentement OAuth rendu côté serveur, sans JavaScript : un
//! formulaire POST vers `/oauth/approve`, qui répond 303 vers le client MCP
//! (code émis ou refus). La session est exigée avant le rendu par une garde
//! serveur qui redirige vers `/login?next=…` — installer un connecteur ne
//! dépend plus du chargement du bundle WASM.
//!
//! Le demandeur se nomme : la query ne porte que le `client_id`, opaque, alors
//! que l'enregistrement (RFC 7591) a déclaré un nom, lu en base au rendu. Ce
//! nom est déclaratif — l'hôte affiché à côté est, lui, la cible réelle du
//! retour, et `approve` refuse tout ce qui n'est pas enregistré pour ce client.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::pages::decision_page::data::sendable;

/// Nom déclaré par un tiers sur un enregistrement ouvert : coupé pour qu'il
/// reste un nom, et non une phrase posée dans la nôtre.
const NOM_MAX: usize = 60;

const FORT: &str = "text-[var(--color-ink)]";

#[component]
pub fn AuthorizeMcpPage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let ui = &crate::i18n::use_ui().account;
    let champ = move |nom: &'static str| query.with(|q| q.get(nom).unwrap_or_default());
    let declare = Resource::new_blocking(move || champ("client_id"), |id| sendable(nom_client(id)));
    let annonce = move |nom: Option<String>| {
        let (fort, hote) = demandeur(nom, &champ("redirect_uri"), ui.external_app);
        view! { <strong class=FORT>{fort}</strong>{hote.map(|hote| format!(" ({hote})"))} }
    };

    view! {
        <Title text=ui.authorize_title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
            <div class="flex flex-col gap-2">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">{ui.authorize_heading}</h1>
                <p class="text-sm text-[var(--color-ink-muted)]">
                    <Suspense fallback=move || annonce(None)>
                        {move || declare.get().map(annonce)}
                    </Suspense>
                    {ui.authorize_asks}
                </p>
            </div>

            <div class="rounded-md border border-[var(--color-rule)] p-4 text-sm text-[var(--color-ink-muted)]">
                <p class="font-medium text-[var(--color-ink)]">{ui.access_granted}</p>
                <ul class="mt-2 list-inside list-disc">
                    {ui.grants.iter().map(|grant| view! { <li>{*grant}</li> }).collect_view()}
                </ul>
            </div>

            <form method="post" action="/oauth/approve" class="flex flex-col gap-3">
                <input type="hidden" name="client_id" value=move || champ("client_id") />
                <input type="hidden" name="redirect_uri" value=move || champ("redirect_uri") />
                <input type="hidden" name="code_challenge" value=move || champ("code_challenge") />
                <input type="hidden" name="state" value=move || champ("state") />
                <button
                    type="submit"
                    name="action"
                    value="approve"
                    class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                >
                    {ui.authorize}
                </button>
                <button
                    type="submit"
                    name="action"
                    value="deny"
                    class="rounded-md border border-[var(--color-rule)] px-4 py-2.5 text-center text-sm text-[var(--color-ink-muted)] transition-colors hover:text-[var(--color-ink)]"
                >
                    {ui.deny}
                </button>
            </form>
            <crate::pages::login_page::SignalerDepuisEcranBloquant de="/authorize-mcp" />
        </div>
    }
}

/// Nom d'enregistrement du client. Lu en base au SSR, comme le profil ; en
/// hydratation la valeur vient du payload sérialisé par la resource bloquante.
async fn nom_client(client_id: String) -> Option<String> {
    #[cfg(feature = "ssr")]
    {
        crate::api::ApiClient::from_context()
            .oauth_client_name(&client_id)
            .await
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = client_id;
        None
    }
}

/// Qui demande, en deux morceaux : le nom mis en avant, et l'hôte de retour
/// entre parenthèses quand un nom l'accompagne.
fn demandeur(nom: Option<String>, redirect_uri: &str, inconnu: &str) -> (String, Option<String>) {
    let hote = hote(redirect_uri);
    match (nom.map(tronque), hote) {
        (Some(nom), hote) => (nom, hote),
        (None, Some(hote)) => (hote, None),
        (None, None) => (inconnu.to_string(), None),
    }
}

/// Hôte du `redirect_uri`, port compris : la cible du 303 après approbation.
fn hote(redirect_uri: &str) -> Option<String> {
    let sans_schema = redirect_uri
        .split_once("://")
        .map_or(redirect_uri, |(_, reste)| reste);
    let hote = sans_schema
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    (!hote.is_empty()).then(|| hote.to_string())
}

fn tronque(nom: String) -> String {
    if nom.chars().count() <= NOM_MAX {
        return nom;
    }
    nom.chars().take(NOM_MAX).chain(['…']).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INCONNU: &str = "Une application externe";

    #[test]
    fn nom_declare_et_hote_de_retour() {
        assert_eq!(
            demandeur(
                Some("Claude".to_string()),
                "https://claude.ai/api/mcp/auth_callback",
                INCONNU
            ),
            ("Claude".to_string(), Some("claude.ai".to_string()))
        );
    }

    #[test]
    fn sans_nom_l_hote_tient_lieu_de_nom() {
        assert_eq!(
            demandeur(None, "http://localhost:8080/cb", INCONNU),
            ("localhost:8080".to_string(), None)
        );
    }

    #[test]
    fn sans_rien_d_exploitable() {
        assert_eq!(demandeur(None, "", INCONNU), (INCONNU.to_string(), None));
    }

    #[test]
    fn nom_kilometrique_coupe() {
        let (nom, _) = demandeur(
            Some("LibreJustice ".repeat(20)),
            "https://claude.ai/cb",
            INCONNU,
        );
        assert_eq!(nom.chars().count(), NOM_MAX + 1);
        assert!(nom.ends_with('…'));
    }
}
