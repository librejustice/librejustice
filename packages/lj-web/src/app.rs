//! Shell HTML + composant racine `App` + table de routage.

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, Title};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::{ParamSegment, SsrMode, StaticSegment};

use crate::components::AppShell;
use crate::pages::{
    ActivityPage, AnnuaireDirectoryPage, AnnuairePage, AuthorizeMcpPage, CodeCataloguePage,
    Confidentialite, DecisionPage, EntityPage, Landing, LawArticlePage, LawCodePage,
    LawComparePage, LawSectionPage, LoginPage, McpGuide, MentionsLegales, NotFound, ProfilePage,
    ResetPasswordPage, SearchPage, SourcesPage, TextesPage,
};
use crate::seo::site_default;

/// Script anti-FOUC, byte-identique a apps/web `root.tsx`. Applique `.dark` avant
/// le premier paint : anonyme (pas de `lj-auth=1`) => toujours light ; connecte
/// => `lj-theme` sinon `prefers-color-scheme`. Pose AVANT
/// HydrationScripts/MetaTags pour s'executer avant l'hydratation.
#[cfg(feature = "ssr")]
const ANTI_FOUC: &str = r#"(() => {
  try {
    if (localStorage.getItem("lj-auth") !== "1") return;
    const v = localStorage.getItem("lj-theme");
    const dark =
      v === "dark" ||
      (v !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
    if (dark) document.documentElement.classList.add("dark");
  } catch {}
})();"#;

/// Config Supabase injectee dans la page pour le shim `js/auth.js` (qui ne peut
/// pas lire `import.meta.env`). Lue via `Settings` (regle repo #5) ; vide si non
/// configuree => auth inerte cote shim. `serde_json` echappe les valeurs.
#[cfg(feature = "ssr")]
fn supabase_config_script() -> String {
    let settings = crate::config::Settings::from_env();
    let payload = serde_json::json!({
        "url": settings.supabase_url,
        "anonKey": settings.supabase_anon_key,
    });
    format!("window.__LJ_SUPABASE__={payload};")
}

/// Document HTML emis par le serveur. cargo-leptos l'utilise pour le SSR et le
/// fallback statique. Aucun `Cache-Control` pose ici (parite entry.server : seul
/// le Content-Type text/html, gere par leptos_axum) — le cache vient de Caddy.
#[cfg(feature = "ssr")]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    use leptos_meta::{HashedStylesheet, MetaTags};
    view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1.0" />
                <meta name="color-scheme" content="light dark" />
                <meta
                    name="theme-color"
                    content="#FBF9F4"
                    media="(prefers-color-scheme: light)"
                />
                <meta
                    name="theme-color"
                    content="#1B1612"
                    media="(prefers-color-scheme: dark)"
                />
                // Set complet : Google sonde /favicon.ico à la racine et veut un
                // raster ≥ 48 px ; l'ICO 16/32/48 + le PNG 192 (rond du SERP sur
                // écrans denses) servent de repli fiable quand le SVG n'est pas
                // (re)pris. apple-touch-icon pour iOS.
                <link rel="icon" href="/favicon.ico" sizes="48x48" />
                <link rel="icon" href="/icon-192.png" type="image/png" sizes="192x192" />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml" sizes="any" />
                <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
                // Précharge la police du LCP (Geist latin, h1 du hero) en
                // parallèle du CSS plutôt qu'après son parsing : casse la chaîne
                // critique HTML → CSS → woff2 qui laissait `font-display: swap`
                // peindre d'abord la fallback (flash de mauvaise police).
                // crossorigin obligatoire : les fonts sont fetchées en mode CORS,
                // sinon le preload ne matche pas la requête réelle (double DL).
                <link
                    rel="preload"
                    href="/fonts/geist-latin-wght-normal.woff2"
                    r#as="font"
                    type="font/woff2"
                    crossorigin="anonymous"
                />
                <script inner_html=supabase_config_script()></script>
                <script inner_html=ANTI_FOUC></script>
                <AutoReload options=options.clone() />
                <HydrationScripts options=options.clone() />
                <HashedStylesheet options id="leptos" />
                <MetaTags />
            </head>
            <body class="antialiased">
                <App />
            </body>
        </html>
    }
}

/// Composant racine : contexte meta, cache de requetes, meta generiques, Router
/// + AppShell.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    crate::query::provide_query_client();
    crate::components::search::compact_search::search_data::provide_seen_search_keys();
    crate::components::client_only::provide_hydrated();
    crate::auth::provide_auth_state();
    provide_auth_runtime();
    let default = site_default();

    view! {
        <Title text=default.title />
        // `<Meta name="description">` generique : pose par `AppShell` (dans le
        // Router) et non ici, pour pouvoir le supprimer sur `/decision/*` ou la
        // page emet sa propre description (parite RR : la route remplace la meta
        // racine, jamais deux balises description). Cf. AppShell.
        <Router>
            <AppShell>
                <Routes fallback=NotFound>
                    // ── Pages publiques (SSR streaming par defaut, OutOfOrder) ──
                    <Route path=StaticSegment("") view=Landing />
                    // ── Recherche (gabarit de référence) : deux pages distinctes —
                    // décisions (`/decisions`, hybride BM25+ANN) et textes
                    // (`/textes`, BM25 seul) : moteurs et filtres propres,
                    // aucune recherche transverse. ──
                    <Route path=StaticSegment("decisions") view=SearchPage />
                    <Route path=StaticSegment("textes") view=TextesPage />
                    <Route path=StaticSegment("sources") view=SourcesPage />
                    // ── Decision : similaires en streaming (PartiallyBlocked :
                    // le bloc bloquant est rendu cote serveur, sans JS). ──
                    <Route
                        path=(StaticSegment("decision"), ParamSegment("id"))
                        view=DecisionPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Fiche entité (ADR 0189) : en-tête + agrégats contentieux
                    // bloquants SSR (SEO) ; liste de décisions citantes streamée
                    // (PartiallyBlocked), paginée par `?page=`. Rendu adapté au
                    // namespace (`siren`/`rna`/`cnb`/`oacc`). ──
                    <Route
                        path=(StaticSegment("entite"), ParamSegment("ns"), ParamSegment("id"))
                        view=EntityPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Annuaire des entités (ADR 0192) : accueil (recherche +
                    // cartes de catégories) et listing paginé par catégorie.
                    // Résultats `?q=` et listing bloquants SSR (crawlables) ;
                    // compteurs des cartes streamés. ──
                    <Route
                        path=StaticSegment("annuaire")
                        view=AnnuairePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    <Route
                        path=(StaticSegment("annuaire"), ParamSegment("kind"))
                        view=AnnuaireDirectoryPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Référentiel LEGI (ADR 0092) : sommaire de code, article
                    // en vigueur, article à une date. L'article (en-tête/méta/
                    // corps/timeline) est bloquant SSR (SEO) ; les décisions
                    // citantes sont streamées (PartiallyBlocked). La date est une
                    // route séparée, pas un segment optionnel. ──
                    // Catalogue des codes (SSR, indexable) : point d'entrée du
                    // référentiel, à côté des routes `/texte`.
                    <Route path=StaticSegment("codes") view=CodeCataloguePage />
                    <Route path=(StaticSegment("texte"), ParamSegment("code")) view=LawCodePage />
                    // Vue-lecture d'une section (ADR 0207) : segment littéral
                    // `section`, déclaré avant la capture `{num}`.
                    <Route
                        path=(
                            StaticSegment("texte"),
                            ParamSegment("code"),
                            StaticSegment("section"),
                            ParamSegment("cid"),
                        )
                        view=LawSectionPage
                    />
                    <Route
                        path=(StaticSegment("texte"), ParamSegment("code"), ParamSegment("num"))
                        view=LawArticlePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    <Route
                        path=(
                            StaticSegment("texte"),
                            ParamSegment("code"),
                            ParamSegment("num"),
                            ParamSegment("date"),
                        )
                        view=LawArticlePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // Comparateur de versions (ADR 0193) : diff serveur entre
                    // deux rédactions, bornes = dates de fenêtre de version.
                    <Route
                        path=(
                            StaticSegment("texte"),
                            ParamSegment("code"),
                            ParamSegment("num"),
                            StaticSegment("comparer"),
                            ParamSegment("de"),
                            ParamSegment("a"),
                        )
                        view=LawComparePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Pages auth client-side : rendu SSR du shell, decision
                    // de session/contenu cote client (AuthGuard / formulaires). ──
                    <Route path=StaticSegment("connexion") view=LoginPage />
                    <Route
                        path=StaticSegment("reinitialiser-mot-de-passe")
                        view=ResetPasswordPage
                    />
                    <Route path=StaticSegment("authorize-mcp") view=AuthorizeMcpPage />
                    <Route path=StaticSegment("profil") view=ProfilePage />
                    // ── Activite : meme composant, une route explicite par
                    // onglet sous `/activite/` (y compris le defaut — si le
                    // defaut change, aucune URL ne change de sens). `/activite`
                    // nu = 308 vers l'onglet recherches (lj-server). ──
                    <Route
                        path=(StaticSegment("activite"), StaticSegment("recherches"))
                        view=ActivityPage
                    />
                    <Route
                        path=(StaticSegment("activite"), StaticSegment("signets"))
                        view=ActivityPage
                    />
                    <Route
                        path=(StaticSegment("activite"), StaticSegment("lectures"))
                        view=ActivityPage
                    />
                    // ── Pages statiques publiques ──
                    <Route path=StaticSegment("mcp-guide") view=McpGuide />
                    <Route path=StaticSegment("mentions-legales") view=MentionsLegales />
                    <Route path=StaticSegment("confidentialite") view=Confidentialite />
                    // Pas de route catch-all : un `WildcardSegment` ici masquerait
                    // les assets statiques (`/pkg/*`, `/favicon.svg`) qui doivent
                    // atteindre le `file_and_error_handler` (fallback axum). Le
                    // no-match est couvert par `fallback=NotFound` (rendu par le
                    // file handler quand aucun fichier ne correspond) ; NotFound
                    // pose lui-même le statut 404 via ResponseOptions.
                </Routes>
            </AppShell>
        </Router>
    }
}

/// Pilote unique de l'etat d'auth cote client (consolide `useAuthEmail` +
/// cache-invalidation + `theme-bridge.ts` du React legacy). Une seule lecture de
/// session + un seul abonnement `onAuthStateChange` alimentent les trois
/// consommateurs :
///   1. le signal `email` de l'`AuthState` (avatar + menu compte de la top-bar) ;
///   2. l'invalidation du cache de requetes par-utilisateur (profil, signets,
///      activite) sur connexion/deconnexion/refresh ;
///   3. le flag `lj-auth` + le theme (cf. ANTI_FOUC : sombre reserve aux
///      connectes, anonyme => toujours clair).
///
/// La sync n'a lieu qu'une fois la session connue (jamais sur un etat « pas
/// encore charge »), donc aucun flash clair→sombre au reload d'un connecte.
#[cfg(feature = "hydrate")]
fn provide_auth_runtime() {
    use crate::components::profile::sync_auth_theme;
    use leptos_fetch::QueryClient;

    let email = crate::auth::use_auth().email;
    let client = expect_context::<QueryClient>();

    // Applique l'etat de session courant. `clear_cache` est faux a la lecture
    // initiale (cache vide au montage) et vrai sur tout (vrai) changement d'auth.
    let apply = move |clear_cache: bool| {
        leptos::task::spawn_local(async move {
            let mail = crate::auth::current_email().await;
            let authed = mail.is_some();
            email.set(mail);
            if clear_cache {
                client.clear();
            }
            sync_auth_theme(authed);
        });
    };

    apply(false);
    // Abonnement permanent : le garde n'est ni `Send` ni `Sync` (il tient un
    // `Closure` JS), inconfiable a un cleanup d'Owner (`Send + Sync`) ; sur wasm
    // mono-thread, `forget` traduit « abonnement vivant le temps de la page ».
    let subscription = crate::auth::on_auth_state_change(move |_event| apply(true));
    std::mem::forget(subscription);
}

/// No-op SSR : pas de session locale ni d'evenement d'auth cote serveur.
#[cfg(feature = "ssr")]
fn provide_auth_runtime() {}
