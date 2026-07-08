// Sentinelle de scroll infini (port de `InfiniteSentinel`). `web-sys` n'expose
// pas `IntersectionObserver` sans feature dans le crate fige : on observe ici en
// JS et on rappelle `cb` quand l'element entre dans le viewport (marge 400px).
// Renvoie une fn de deconnexion.
export function observeIntersection(node, cb) {
  if (typeof window === "undefined" || node === null) {
    return () => {};
  }
  const observer = new IntersectionObserver(
    (entries) => {
      if (entries[0]?.isIntersecting) {
        cb();
      }
    },
    { rootMargin: "400px 0px" },
  );
  observer.observe(node);
  return () => observer.disconnect();
}
