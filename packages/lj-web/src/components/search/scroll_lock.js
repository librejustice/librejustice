// Anti scroll-chaining d'un conteneur scrollable (port de `ScrollContainer`).
// Un listener `wheel` non-passif (`{passive:false}`, requis pour pouvoir
// `preventDefault`) bloque la propagation du scroll a la page quand on est en
// butee (haut + molette vers le haut, ou bas + molette vers le bas). `web-sys`
// n'exprime pas proprement un listener non-passif : on le pose ici en JS.
// Renvoie une fn de deconnexion.
export function lockScrollChaining(el) {
  if (typeof window === "undefined" || el === null) {
    return () => {};
  }
  const onWheel = (e) => {
    const { scrollTop, scrollHeight, clientHeight } = el;
    if (scrollHeight <= clientHeight) {
      return;
    }
    const atTop = scrollTop === 0 && e.deltaY < 0;
    const atBottom = scrollTop + clientHeight >= scrollHeight - 1 && e.deltaY > 0;
    if (atTop || atBottom) {
      e.preventDefault();
    }
  };
  el.addEventListener("wheel", onWheel, { passive: false });
  return () => el.removeEventListener("wheel", onWheel);
}
