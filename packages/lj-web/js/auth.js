// Shim d'authentification borne + helpers navigateur (port de
// apps/web/src/lib/supabase.ts et de divers effets DOM des pages auth). Un seul
// module, un seul bootstrap d'AuthClient pour toutes les ops auth du front.
//
// Bundle par wasm-bindgen (`#[wasm_bindgen(module = "/js/auth.js")]`) : ce
// fichier devient un "snippet" inline dans le bundle wasm, charge par le
// glue-code JS de cargo-leptos. Il N'a PAS acces a `import.meta.env` (pas de
// passe Vite) — la config Supabase est injectee au SSR dans
// `window.__LJ_SUPABASE__` (cf. app.rs::shell), avec fallback sur des
// placeholders (auth inerte).
//
// On instancie AuthClient (@supabase/auth-js) directement plutot que
// createClient() : pas besoin de postgrest/realtime/storage/functions cote
// front. Module vendore (public/vendor/, bundle esm.sh epingle v2.107.0)
// importe par chemin ABSOLU : wasm-bindgen inline ce shim mais ne suit pas ses
// `import` (pas de bundler npm dans la chaine wasm) — la lib est donc servie en
// asset statique et chargee au runtime, pas bundlee. Bindings cote Rust :
// `src/auth/bindings.rs` (substrate auth) et `src/pages/browser.rs` (ops auth
// page-level + helpers DOM).
import { AuthClient } from "/vendor/supabase-auth-js.js";

function config() {
  const c = (typeof window !== "undefined" && window.__LJ_SUPABASE__) || {};
  return { url: c.url || "", anonKey: c.anonKey || "" };
}

// Reproduit la storageKey par defaut de createClient(supabase-js) afin de
// preserver les sessions existantes : "sb-<projectref>-auth-token".
function deriveStorageKey(url) {
  const match = url.match(/^https?:\/\/([^.]+)/);
  return `sb-${match ? match[1] : "default"}-auth-token`;
}

// AuthClient instancie paresseusement : son constructeur touche `localStorage`
// (persistSession), indisponible cote SSR. On ne l'instancie qu'au premier
// acces, qui n'arrive que cote navigateur.
let _client = null;

function authClient() {
  if (_client) {
    return _client;
  }
  const { url, anonKey } = config();
  const effectiveUrl = url || "http://placeholder.local";
  const effectiveKey = anonKey || "placeholder";
  _client = new AuthClient({
    url: `${effectiveUrl}/auth/v1`,
    headers: {
      Authorization: `Bearer ${effectiveKey}`,
      apikey: effectiveKey,
    },
    storageKey: deriveStorageKey(effectiveUrl),
    autoRefreshToken: true,
    persistSession: true,
    detectSessionInUrl: true,
    flowType: "pkce",
  });
  return _client;
}

function configured() {
  const { url, anonKey } = config();
  return Boolean(url && anonKey);
}

// --- API auth (bindee dans src/auth/bindings.rs + src/pages/browser.rs) ------

// Jeton d'acces de la session courante, ou null (anonyme / non configure / SSR).
export async function getAccessToken() {
  if (typeof window === "undefined" || !configured()) {
    return null;
  }
  const {
    data: { session },
  } = await authClient().getSession();
  return session?.access_token ?? null;
}

// Email de l'utilisateur de la session courante, ou null (anonyme / non
// configure / SSR). Utilise par l'etat d'auth reactif de la top-bar.
export async function getUserEmail() {
  if (typeof window === "undefined" || !configured()) {
    return null;
  }
  const {
    data: { session },
  } = await authClient().getSession();
  return session?.user?.email ?? null;
}

// `true` si une session existe (utilise par l'AuthGuard).
export async function hasSession() {
  if (typeof window === "undefined" || !configured()) {
    return false;
  }
  const {
    data: { session },
  } = await authClient().getSession();
  return Boolean(session);
}

// Connexion email/mot de passe. Rejette avec l'AuthError supabase brut.
export async function signInPassword(email, password) {
  const { error } = await authClient().signInWithPassword({ email, password });
  if (error) {
    throw error;
  }
}

// Connexion OAuth (PKCE). `provider` ex. "google". `redirectTo` URL de retour.
export async function signInOauth(provider, redirectTo) {
  const { error } = await authClient().signInWithOAuth({
    provider,
    options: { redirectTo },
  });
  if (error) {
    throw error;
  }
}

// Inscription email/mot de passe. `redirectTo` = lien de confirmation.
export async function signUp(email, password, redirectTo) {
  const { error } = await authClient().signUp({
    email,
    password,
    options: { emailRedirectTo: redirectTo },
  });
  if (error) {
    throw error;
  }
}

// Demande de reinitialisation de mot de passe (email de reset).
export async function resetPassword(email, redirectTo) {
  const { error } = await authClient().resetPasswordForEmail(email, { redirectTo });
  if (error) {
    throw error;
  }
}

// Deconnexion.
export async function signOut() {
  await authClient().signOut();
}

// Renvoi de l'email de confirmation (type "signup"). Rejette avec l'AuthError.
export async function resendSignup(email) {
  const { error } = await authClient().resend({ type: "signup", email });
  if (error) {
    throw error;
  }
}

// Mise a jour du mot de passe de l'utilisateur courant. Rejette avec l'AuthError.
export async function updatePassword(password) {
  const { error } = await authClient().updateUser({ password });
  if (error) {
    throw error;
  }
}

// Abonnement aux changements d'etat d'auth. `cb` est appele a chaque evenement
// (SIGNED_IN / SIGNED_OUT / TOKEN_REFRESHED / PASSWORD_RECOVERY…) avec le nom de
// l'evenement. Renvoie une fn de desabonnement.
export function onAuthStateChange(cb) {
  if (typeof window === "undefined" || !configured()) {
    return () => {};
  }
  const {
    data: { subscription },
  } = authClient().onAuthStateChange((event) => cb(event));
  return () => subscription.unsubscribe();
}

// --- Helpers DOM (web-sys indisponible sans features) -----------------------

// Origine courante (`window.location.origin`).
export function locationOrigin() {
  return typeof window === "undefined" ? "" : window.location.origin;
}

// Chemin + query courant (`pathname + search`).
export function locationPathSearch() {
  if (typeof window === "undefined") {
    return "/";
  }
  return `${window.location.pathname}${window.location.search}`;
}

// Hash courant (`window.location.hash`).
export function locationHash() {
  return typeof window === "undefined" ? "" : window.location.hash;
}

// Efface le hash en preservant pathname + search (history.replaceState).
export function clearLocationHash() {
  if (typeof window === "undefined") {
    return;
  }
  if (window.location.hash) {
    history.replaceState(null, "", window.location.pathname + window.location.search);
  }
}

// Navigation native dure (window.location.href = url).
export function navigateHard(url) {
  if (typeof window !== "undefined") {
    window.location.href = url;
  }
}

// Confirmation native (window.confirm).
export function confirmNative(message) {
  return typeof window === "undefined" ? false : window.confirm(message);
}

// Construit une URL en posant des parametres de query, renvoie sa string. Port
// de `new URL(base); url.searchParams.set(...)`.
export function buildUrlWithParams(base, pairsJson) {
  const url = new URL(base);
  const pairs = JSON.parse(pairsJson);
  for (const [k, v] of pairs) {
    if (v !== null && v !== undefined) {
      url.searchParams.set(k, v);
    }
  }
  return url.toString();
}

// POST JSON avec bearer (consentement OAuth /oauth/approve). Renvoie le `code`
// renvoye par l'API. Rejette avec le texte d'erreur si `!res.ok` (port fidele de
// authorize-mcp-page.tsx::handleApprove).
export async function oauthApprove(baseUrl, token, bodyJson) {
  const res = await fetch(`${baseUrl}/oauth/approve`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: bodyJson,
  });
  if (!res.ok) {
    throw new Error(await res.text());
  }
  const { code } = await res.json();
  return code;
}
