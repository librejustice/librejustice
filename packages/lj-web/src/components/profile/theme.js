// Helpers DOM du selecteur de theme (port de la logique localStorage /
// matchMedia / classList de `theme-settings.tsx`). `web-sys` n'a aucune feature
// activee dans le crate fige : on passe par ce shim JS minimal.

const STORAGE_KEY = "lj-theme";
const AUTH_FLAG = "lj-auth";

// Lit le theme persiste : "light" | "dark" | "system" (defaut).
export function readTheme() {
  if (typeof window === "undefined") {
    return "system";
  }
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "light" || v === "dark") {
      return v;
    }
  } catch {}
  return "system";
}

// Persiste le theme : "system" => remove ; sinon set.
export function persistTheme(theme) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    if (theme === "system") {
      localStorage.removeItem(STORAGE_KEY);
    } else {
      localStorage.setItem(STORAGE_KEY, theme);
    }
  } catch {}
}

// Applique le theme : toggle `.dark` selon dark | (system && prefers-dark).
export function applyTheme(theme) {
  if (typeof window === "undefined") {
    return;
  }
  const isDark =
    theme === "dark" ||
    (theme === "system" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", isDark);
}

// Synchronise le flag d'auth `lj-auth` puis (ré)applique le thème en
// conséquence. Sémantique (cf. anti-FOUC d'app.rs) : le thème sombre est réservé
// aux connectés — anonyme => flag retiré + thème clair forcé ; connecté => flag
// posé + thème `lj-theme` (sinon `prefers-color-scheme`). Port de
// `theme-bridge.ts` (apps/web legacy).
export function syncAuthTheme(authenticated) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    if (authenticated) {
      localStorage.setItem(AUTH_FLAG, "1");
    } else {
      localStorage.removeItem(AUTH_FLAG);
    }
  } catch {}
  if (authenticated) {
    applyTheme(readTheme());
  } else {
    document.documentElement.classList.remove("dark");
  }
}

// Abonne `cb` aux changements de `prefers-color-scheme`. Renvoie le desabonnement.
export function onPrefersDarkChange(cb) {
  if (typeof window === "undefined") {
    return () => {};
  }
  const mq = matchMedia("(prefers-color-scheme: dark)");
  const handler = () => cb();
  mq.addEventListener("change", handler);
  return () => mq.removeEventListener("change", handler);
}
