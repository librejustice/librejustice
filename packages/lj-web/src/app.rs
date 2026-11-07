//! Shell HTML + composant racine `App` + table de routage.

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, Title};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::{ParamSegment, SsrMode, StaticSegment, WildcardSegment};

use crate::components::AppShell;
use crate::pages::{
    AnnoncesCataloguePage, AnnoncesDepartementPage, AnnoncesPage, AnnuaireDirectoryPage,
    AnnuairePage, ApiGuide, AuthorizeMcpPage, BookmarksPage, CodeCataloguePage, ConfirmPage,
    ContributePage, CorpusPage, DecisionPage, EmailChangePage, EntityPage, FeedbackPage,
    JuridictionHubPage, JuridictionYearPage, JuridictionsPage, Landing, LawCodePage, LegalNotice,
    LoginLinkPage, LoginPage, MarchePage, McpGuide, NormFondPage, NormNodePage, NormYearPage,
    NormesPage, NotFound, OpenSource, Privacy, ProfilePage, RedactPage, ResetPasswordPage,
    SearchPage, SourcesPage, Terms, TextesPage, ToolsPage,
};
use crate::seo::site_default;

/// Script anti-FOUC. Applique `.dark` avant
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

/// Bandeau bas partagé, et suivi du build servi.
///
/// `__ljBar(message, refermable)` pose le seul recours possible quand la page ne
/// peut pas se réparer seule : un message et un bouton qui recharge.
///
/// `__ljBuild(id)` reçoit l'identité du bundle portée par les réponses `/internal`
/// (le client `hydrate` la lui passe). Elle est mémorisée à la
/// première réponse et jamais réécrite ; si une suivante en porte une autre, un
/// déploiement a eu lieu sous l'onglet et le bandeau le dit — **une fois**, la
/// suite étant close définitivement. Aucun sondage : le suivi voyage sur le
/// trafic existant.
#[cfg(feature = "ssr")]
const BANDEAU: &str = r#"(() => {
  const bouton =
    "padding:.35rem .9rem;border:1px solid currentColor;border-radius:4px;" +
    "background:transparent;color:inherit;cursor:pointer;font:inherit";

  window.__ljBar = (message, refermable) => {
    const bar = document.createElement("div");
    bar.setAttribute("role", "alert");
    bar.style.cssText =
      "position:fixed;left:0;right:0;bottom:0;z-index:9999;display:flex;flex-wrap:wrap;" +
      "gap:1rem;align-items:center;justify-content:center;padding:.75rem 1rem;" +
      "background:#1B1612;color:#FBF9F4;font:14px/1.4 system-ui,sans-serif";
    const text = document.createElement("span");
    text.textContent = message;
    const reload = document.createElement("button");
    reload.textContent = "Recharger";
    reload.style.cssText = bouton;
    reload.onclick = () => location.reload();
    bar.append(text, reload);
    if (refermable) {
      const plus_tard = document.createElement("button");
      plus_tard.textContent = "Plus tard";
      plus_tard.style.cssText = bouton + ";border-color:transparent;opacity:.7";
      plus_tard.onclick = () => bar.remove();
      bar.append(plus_tard);
    }
    document.body.appendChild(bar);
  };

  let vu = null;
  let annonce = false;
  window.__ljBuild = (id) => {
    if (annonce || !id) return;
    if (vu === null) {
      vu = id;
    } else if (vu !== id) {
      annonce = true;
      window.__ljBar("Une nouvelle version est disponible.", true);
    }
  };
})();"#;

#[cfg(feature = "ssr")]
const SONDE_CLIENT: &str = r#"(() => {
  window.__ljIncident = (motif, detail) => {
    try {
      const body = JSON.stringify({
        kind: motif,
        path: location.pathname,
        ...detail,
      });
      navigator.sendBeacon(
        "/internal/client/incident",
        new Blob([body], { type: "application/json" }),
      );
    } catch {}
  };

  const broken = new Map();
  document.addEventListener(
    "error",
    (e) => {
      const target = e.target;
      const url = target && target.tagName ? String(target.href || target.src || "") : "";
      if (url.includes("/pkg/")) broken.set(url.split("/").pop(), 0);
    },
    true,
  );

  const failedAssets = () => {
    const seen = new Map(broken);
    for (const r of performance.getEntriesByType("resource")) {
      if (r.name.includes("/pkg/") && r.responseStatus >= 400) {
        seen.set(r.name.split("/").pop(), r.responseStatus);
      }
    }
    return Array.from(seen, ([name, status]) => name + ":" + status);
  };

  const styleAbsent = () => {
    const lien = document.getElementById("leptos");
    if (!lien) return false;
    try {
      return !lien.sheet || lien.sheet.cssRules.length === 0;
    } catch {
      return false;
    }
  };

  let masque = document.visibilityState === "hidden";
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) masque = true;
  });

  const check = (elapsed) => {
    if (window.__LJ_HYDRATED) {
      const nu = styleAbsent();
      window.__ljIncident(nu ? "stylesheet_missing" : "hydration_ok", {
        assets: nu ? failedAssets() : [],
        elapsed_ms: Math.round(window.__LJ_HYDRATED_AT ?? performance.now()),
      });
      return;
    }
    const assets = failedAssets();
    if (assets.length === 0 && elapsed < 30000) {
      setTimeout(() => check(elapsed + 5000), 5000);
      return;
    }
    if (!masque) {
      window.__ljIncident("hydration", {
        assets,
        stylesheet_missing: styleAbsent(),
        elapsed_ms: Math.round(performance.now()),
      });
    }
    window.__ljBar("Cette page n'a pas pu charger entièrement.", false);
  };

  setTimeout(() => check(5000), 5000);
})();"#;

/// Content-Security-Policy du document HTML.
///
/// Forme « strict CSP » : les scripts inline ne s'exécutent que sous le `nonce`
/// de la requête, celui que `leptos_axum` provisionne et que `HydrationScripts`
/// pose déjà sur le bootstrap d'hydratation. Les trois scripts inline du `<head>`
/// portent le même. `frame-ancestors 'none'` interdit d'encadrer
/// `/authorize-mcp`, dont un clic de consentement piégé accorderait l'accès MCP
/// au compte de la victime.
///
/// Pas de `default-src` : seules sont posées les directives dont l'inventaire
/// des ressources est vérifié, toutes same-origin.
///
/// - `'wasm-unsafe-eval'` : sans lui le bundle `hydrate` ne compile pas ;
/// - `style-src 'unsafe-inline'` : le rendu émet des attributs `style=`.
///
/// Sur une page cachée le `nonce` voyage dans le cache CDN, donc public. Ce qui
/// protège alors : `script-src 'self'`, `object-src`, `base-uri`, `form-action`
/// et `frame-ancestors`.
#[cfg(feature = "ssr")]
fn content_security_policy(nonce: Option<&str>) -> String {
    let settings = crate::config::Settings::from_env();
    let script_nonce = match nonce {
        Some(n) => format!(" 'nonce-{n}'"),
        None => String::new(),
    };
    let mut connect_extra = String::new();
    if settings.watch {
        connect_extra.push_str(" ws://localhost:* ws://127.0.0.1:*");
    }

    [
        format!("script-src 'self' 'wasm-unsafe-eval'{script_nonce}"),
        "style-src 'self' 'unsafe-inline'".to_string(),
        "img-src 'self' data:".to_string(),
        "font-src 'self'".to_string(),
        "media-src 'self'".to_string(),
        format!("connect-src 'self'{connect_extra}"),
        "object-src 'none'".to_string(),
        "base-uri 'self'".to_string(),
        "form-action 'self'".to_string(),
        "frame-ancestors 'none'".to_string(),
    ]
    .join("; ")
}

/// Document HTML emis par le serveur. cargo-leptos l'utilise pour le SSR et le
/// fallback statique. Aucun `Cache-Control` pose ici (seul le Content-Type
/// text/html, gere par leptos_axum).
#[cfg(feature = "ssr")]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    use leptos_meta::{HashedStylesheet, MetaTags};

    // Nonce de la requête (provisionné par `leptos_axum`) : l'en-tête et les
    // scripts inline ci-dessous doivent venir du même tirage, d'où la lecture
    // unique ici.
    use axum::http::{header::CONTENT_SECURITY_POLICY, HeaderValue};
    let nonce = leptos::nonce::use_nonce().map(|n| n.to_string());
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        if let Ok(hv) = HeaderValue::from_str(&content_security_policy(nonce.as_deref())) {
            resp.insert_header(CONTENT_SECURITY_POLICY, hv);
        }
    }

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
                // raster ≥ 48 px ; l'ICO 16/32/48 + le PNG 192 servent de repli
                // quand le SVG n'est pas repris. apple-touch-icon pour iOS.
                <link rel="icon" href="/favicon.ico" sizes="48x48" />
                <link rel="icon" href="/icon-192.png" type="image/png" sizes="192x192" />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml" sizes="any" />
                <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
                // Précharge la police du LCP (Geist latin, h1 du hero) en
                // parallèle du CSS plutôt qu'après son parsing. `crossorigin`
                // obligatoire : les fonts sont fetchées en mode CORS, sinon le
                // preload ne matche pas la requête réelle.
                <link
                    rel="preload"
                    href="/fonts/geist-latin-wght-normal.woff2"
                    r#as="font"
                    type="font/woff2"
                    crossorigin="anonymous"
                />
                <script nonce=nonce.clone() inner_html=ANTI_FOUC></script>
                <script nonce=nonce.clone() inner_html=BANDEAU></script>
                <script nonce=nonce.clone() inner_html=SONDE_CLIENT></script>
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
    crate::components::search::provide_card_view_pref();
    crate::components::client_only::provide_hydrated();
    crate::auth::provide_auth_state();
    provide_auth_runtime();
    let default = site_default();

    view! {
        <Title text=default.title />
        // `<Meta name="description">` generique : pose par `AppShell`, dans le
        // Router, pour que `/decision/*` puisse emettre la sienne a la place (la
        // route remplace la meta racine, jamais deux balises description).
        <Router>
            <AppShell>
                <Routes fallback=NotFound>
                    // ── Pages publiques (SSR streaming par defaut, OutOfOrder) ──
                    <Route path=StaticSegment("") view=Landing />
                    // Search: `/decision-search` (hybrid BM25+ANN) and `/norm-search`
                    // (BM25 only), their own engines and filters, no cross search.
                    <Route path=StaticSegment("decision-search") view=SearchPage />
                    <Route path=StaticSegment("norm-search") view=TextesPage />
                    <Route path=StaticSegment("sources") view=SourcesPage />
                    // ── Annonces : la publicité légale en cours, plusieurs fonds derrière
                    // un même DTO (marchés publics en base, BODACC en proxy) ; les pages
                    // d'index restent du SSR bloquant, pour le crawl. ──
                    <Route path=StaticSegment("notice-search") view=AnnoncesPage />
                    <Route
                        path=(StaticSegment("notice-index"), ParamSegment("juris"))
                        view=AnnoncesCataloguePage
                    />
                    <Route
                        path=(
                            StaticSegment("notice-index"),
                            ParamSegment("juris"),
                            ParamSegment("code"),
                        )
                        view=AnnoncesDepartementPage
                    />
                    <Route
                        path=(StaticSegment("notice"), ParamSegment("juris"), ParamSegment("uid"))
                        view=MarchePage
                    />
                    // ── Decision : similaires en streaming (PartiallyBlocked :
                    // le bloc bloquant est rendu cote serveur, sans JS). ──
                    <Route
                        path=(StaticSegment("decision"), ParamSegment("juris"), ParamSegment("id"))
                        view=DecisionPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Fiche entité : en-tête et agrégats bloquants SSR, liste de
                    // décisions citantes streamée et paginée par `?page=`. Rendu adapté au
                    // namespace (`siren`/`rna`/`cnb`/`oacc`). ──
                    <Route
                        path=(
                            StaticSegment("entity"),
                            ParamSegment("juris"),
                            ParamSegment("ns"),
                            ParamSegment("id"),
                        )
                        view=EntityPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Annuaire des entités : accueil et listing paginé par catégorie,
                    // bloquants SSR ; compteurs des cartes streamés. ──
                    <Route
                        path=StaticSegment("entity-search")
                        view=AnnuairePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    <Route
                        path=(StaticSegment("entity-index"), ParamSegment("juris"), ParamSegment("kind"))
                        view=AnnuaireDirectoryPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Hubs juridiction : catalogue par famille, hub d'une juridiction
                    // (années), liste paginée d'une année. SSR bloquant — le contenu est
                    // fait de liens crawlables. ──
                    <Route
                        path=(StaticSegment("decision-index"), ParamSegment("juris"))
                        view=JuridictionsPage
                    />
                    <Route
                        path=(StaticSegment("decision-index"), ParamSegment("juris"), ParamSegment("code"))
                        view=JuridictionHubPage
                    />
                    <Route
                        path=(
                            StaticSegment("decision-index"),
                            ParamSegment("juris"),
                            ParamSegment("code"),
                            ParamSegment("annee"),
                        )
                        view=JuridictionYearPage
                    />
                    // ── Hubs normes : même arborescence vers les textes, l'année pouvant
                    // être `sans-date`. ──
                    // What applies in a place: the date is a segment, the place
                    // a setting, as on the article page.
                    <Route
                        path=(StaticSegment("norm-index"), ParamSegment("juris"), StaticSegment("in-force"))
                        view=CorpusPage
                    />
                    <Route
                        path=(
                            StaticSegment("norm-index"),
                            ParamSegment("juris"),
                            StaticSegment("in-force"),
                            ParamSegment("date"),
                        )
                        view=CorpusPage
                    />
                    <Route
                        path=(StaticSegment("norm-index"), ParamSegment("juris"))
                        view=NormesPage
                    />
                    // Le fond `codes` a son propre catalogue (sommaire par
                    // code, pas un hub année) — segment statique prioritaire
                    // sur la capture `{fond}`.
                    <Route
                        path=(StaticSegment("norm-index"), ParamSegment("juris"), StaticSegment("codes"))
                        view=CodeCataloguePage
                    />
                    <Route
                        path=(StaticSegment("norm-index"), ParamSegment("juris"), ParamSegment("fond"))
                        view=NormFondPage
                    />
                    <Route
                        path=(
                            StaticSegment("norm-index"),
                            ParamSegment("juris"),
                            ParamSegment("fond"),
                            ParamSegment("annee"),
                        )
                        view=NormYearPage
                    />
                    // ── LEGI referential: code summary, article in force, article at a
                    // date (a route of its own, not an optional segment). The article
                    // blocks SSR, the citing decisions stream. The first segment names
                    // the enacting jurisdiction; the older shape without it is
                    // redirected by the server, never routed here. ──
                    <Route
                        path=(StaticSegment("norm"), ParamSegment("juris"), ParamSegment("code"))
                        view=LawCodePage
                    />
                    // Every node of a text: an article, at a date or compared, a
                    // section path; `NormNodePage` reads the segments.
                    <Route
                        path=(
                            StaticSegment("norm"),
                            ParamSegment("juris"),
                            ParamSegment("code"),
                            WildcardSegment("rest"),
                        )
                        view=NormNodePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Pages auth et compte : formulaires rendus serveur, sans JS.
                    // Le profil lit son compte au rendu (resource bloquante), derriere
                    // la garde de session serveur. Les anciennes routes en francais
                    // redirigent 308 (lj-server). ──
                    <Route path=StaticSegment("login") view=LoginPage />
                    <Route path=StaticSegment("reset-password") view=ResetPasswordPage />
                    <Route path=StaticSegment("confirm") view=ConfirmPage />
                    <Route path=StaticSegment("login-link") view=LoginLinkPage />
                    <Route path=StaticSegment("email-change") view=EmailChangePage />
                    <Route path=StaticSegment("authorize-mcp") view=AuthorizeMcpPage />
                    <Route
                        path=StaticSegment("profile")
                        view=ProfilePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    <Route path=StaticSegment("bookmarks") view=BookmarksPage />
                    <Route
                        path=(StaticSegment("decision"), StaticSegment("contribute"))
                        view=ContributePage
                    />
                    // ── Pages statiques publiques ──
                    <Route path=StaticSegment("feedback") view=FeedbackPage />
                    <Route path=StaticSegment("tools") view=ToolsPage />
                    <Route path=StaticSegment("redact") view=RedactPage />
                    <Route path=StaticSegment("api-guide") view=ApiGuide />
                    <Route path=StaticSegment("mcp-guide") view=McpGuide />
                    <Route path=StaticSegment("legal-notice") view=LegalNotice />
                    <Route path=StaticSegment("privacy") view=Privacy />
                    <Route path=StaticSegment("terms") view=Terms />
                    <Route path=StaticSegment("open-source") view=OpenSource />
                    // Pas de route catch-all : un `WildcardSegment` masquerait les assets
                    // statiques (`/pkg/*`, `/favicon.svg`) qui doivent atteindre le
                    // `file_and_error_handler`. Le no-match est couvert par
                    // `fallback=NotFound`, qui pose lui-même le 404.
                </Routes>
            </AppShell>
        </Router>
    }
}

/// Pilote unique de l'etat d'auth cote client. Une seule lecture de session +
/// un seul abonnement `onAuthStateChange` alimentent les trois consommateurs :
///   1. le signal `email` de l'`AuthState` (avatar + menu compte de la top-bar) ;
///   2. l'invalidation du cache de requetes sur changement REEL d'identite
///      (connexion, deconnexion, bascule de compte) ;
///   3. le flag `lj-auth` + le theme (cf. ANTI_FOUC : sombre reserve aux
///      connectes, anonyme => toujours clair).
///
/// La sync n'a lieu qu'une fois la session connue (jamais sur un etat « pas
/// encore charge »), donc aucun flash clair→sombre au reload d'un connecte.
#[cfg(feature = "hydrate")]
fn provide_auth_runtime() {
    use crate::components::profile::sync_auth_theme;
    use leptos_fetch::QueryClient;

    let auth = crate::auth::use_auth();
    let email = auth.email;
    let client = expect_context::<QueryClient>();
    // Derniere identite RESOLUE : `None` = jamais lue encore, `Some(None)` =
    // anonyme, `Some(Some(mail))` = connecte. Porte la decision de vider le cache,
    // qui se deduit de la donnee et non du site d'appel.
    let known: StoredValue<Option<Option<String>>> = StoredValue::new(None);

    // Applique l'etat de session courant, et ne vide le cache que sur un vrai
    // changement d'identite (connexion, deconnexion, bascule de compte).
    let apply = move || {
        leptos::task::spawn_local(async move {
            let mail = crate::auth::current_email().await;
            let authed = mail.is_some();
            let changed = matches!(known.get_value(), Some(previous) if previous != mail);
            known.set_value(Some(mail.clone()));
            email.set(mail);
            if changed {
                client.clear();
            }
            sync_auth_theme(authed);
        });
    };

    // Une lecture au montage, puis une par changement annonce (deconnexion).
    Effect::new(move |_| {
        auth.revision.track();
        apply();
    });
}

/// No-op SSR : pas de session locale ni d'evenement d'auth cote serveur.
#[cfg(feature = "ssr")]
fn provide_auth_runtime() {}
