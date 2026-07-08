// Shim presse-papier : `navigator.clipboard` n'est pas exposé par les features
// web-sys héritées de leptos. Bindé via wasm-bindgen (`crate::dom`), comme
// `js/auth.js` / `components/activity/sentinel.js`. Renvoie une Promise qui
// résout au succès, rejette si l'écriture est refusée (permission, focus…).
export function copyText(text) {
  return navigator.clipboard.writeText(text);
}
