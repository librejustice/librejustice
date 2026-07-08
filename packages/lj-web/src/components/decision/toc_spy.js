// Plomberie DOM du scroll-spy du sommaire (port de `decision-toc.tsx`). La
// resolution (section active + barre) reste en Rust (`resolve_scroll_spy`) :
// ce shim ne fait que mesurer le DOM et renvoyer des `Float64Array` paralleles
// a `sectionIds`. `web-sys` n'expose pas `getBoundingClientRect`/`pushState`/
// `scrollIntoView` lisse sans elargir ses features, d'ou ce JS.

// Mesure les ancres absolues et les centres d'items pour chaque id.
// `anchorTops[i]` = top absolu de la section `sectionIds[i]` ; `centers[i]` =
// centre vertical de l'item correspondant, relatif a la liste (comme
// `buildMetrics`). Les ids sans ancre ou sans item gardent `NaN` (filtres cote
// Rust). Renvoie `{ anchorTops, centers }`.
function measure(listEl, sectionIds) {
  const listRect = listEl.getBoundingClientRect();
  const items = listEl.children;
  const count = sectionIds.length;
  const anchorTops = new Float64Array(count);
  const centers = new Float64Array(count);
  for (let i = 0; i < count; i += 1) {
    const anchor = document.getElementById(sectionIds[i]);
    const item = items[i];
    if (!anchor || !item) {
      anchorTops[i] = Number.NaN;
      centers[i] = Number.NaN;
      continue;
    }
    anchorTops[i] = anchor.getBoundingClientRect().top + window.scrollY;
    const itemRect = item.getBoundingClientRect();
    centers[i] = itemRect.top - listRect.top + itemRect.height / 2;
  }
  return { anchorTops, centers };
}

// `markerY`/`atBottom` pour une position de scroll donnee. `maxScroll` est le
// seul endroit qui connait la hauteur du document, d'ou le calcul ici.
function probe(scrollY, markerOffset) {
  const maxScroll = document.documentElement.scrollHeight - window.innerHeight;
  const atBottom = maxScroll > 0 && scrollY >= maxScroll - 2;
  return { markerY: scrollY + markerOffset, atBottom, maxScroll };
}

// Attache `scroll` (passif) + `resize`. A chaque event, mesure et appelle
// `onResolve(markerY, anchorTops, centers, atBottom)`. Le scroll libre rappelle
// d'abord `onAnimateOff()` (suivi 1:1 sans transition). Pendant un scroll
// programme (apres `scrollToSection`), on gele le spy et on relache apres un
// debounce de 140 ms sans nouvel event. Renvoie une fn de deconnexion.
export function observeScrollSpy(listEl, sectionIds, markerOffset, onResolve, onAnimateOff) {
  if (typeof window === "undefined" || listEl === null) {
    return () => {};
  }
  const state = { programmatic: false, settle: null };

  const resolveNow = () => {
    const { markerY, atBottom } = probe(window.scrollY, markerOffset);
    const { anchorTops, centers } = measure(listEl, sectionIds);
    onResolve(markerY, anchorTops, centers, atBottom);
  };

  const release = () => {
    state.programmatic = false;
    if (state.settle !== null) {
      window.clearTimeout(state.settle);
      state.settle = null;
    }
    resolveNow();
  };

  const onScroll = () => {
    if (state.programmatic) {
      if (state.settle !== null) {
        window.clearTimeout(state.settle);
      }
      state.settle = window.setTimeout(release, 140);
      return;
    }
    onAnimateOff();
    resolveNow();
  };

  // Expose l'entree en mode programme au binding `scrollToSection` via le state
  // capture (le binding rappelle `freeze`/`armSafetyNet` plus bas).
  state.freeze = () => {
    state.programmatic = true;
  };
  state.armSafetyNet = () => {
    if (state.settle !== null) {
      window.clearTimeout(state.settle);
    }
    state.settle = window.setTimeout(() => {
      state.programmatic = false;
      state.settle = null;
    }, 700);
  };

  resolveNow();
  window.addEventListener("scroll", onScroll, { passive: true });
  window.addEventListener("resize", onScroll);

  const disconnect = () => {
    window.removeEventListener("scroll", onScroll);
    window.removeEventListener("resize", onScroll);
    if (state.settle !== null) {
      window.clearTimeout(state.settle);
      state.settle = null;
    }
  };
  // On renvoie un objet portant `disconnect` + le `state` partage, pour que
  // `scrollToSection` puisse geler le meme observateur.
  disconnect.state = state;
  return disconnect;
}

// Clic sur une entree (port de `onSectionClick`). Pose l'etat EXACT du point
// d'atterrissage AVANT le scroll via `onLanding(markerY, anchorTops, centers,
// atBottom)` (meme resolveur que le spy → aucun saut), met a jour le hash, puis
// scroll lisse jusqu'a la cible. `handle` est le retour d'`observeScrollSpy`
// (porte le `state` a geler). Filet de securite : si la cible est deja visible,
// aucun `scroll` ne se declenche → relache le gel apres 700 ms.
export function scrollToSection(id, listEl, sectionIds, markerOffset, anchorMargin, handle, onLanding) {
  if (typeof window === "undefined" || listEl === null) {
    return;
  }
  if (handle && handle.state) {
    handle.state.freeze();
  }
  const { anchorTops, centers } = measure(listEl, sectionIds);
  const index = sectionIds.indexOf(id);
  const maxScroll = document.documentElement.scrollHeight - window.innerHeight;
  const anchorTop = index >= 0 ? anchorTops[index] : Number.NaN;
  if (Number.isFinite(anchorTop)) {
    const landedScrollY = Math.min(
      Math.max(anchorTop - anchorMargin, 0),
      Math.max(maxScroll, 0),
    );
    onLanding(landedScrollY + markerOffset, anchorTops, centers, landedScrollY >= maxScroll - 2 && maxScroll > 0);
  }
  if (window.location.hash !== `#${id}`) {
    window.history.pushState(window.history.state, "", `#${id}`);
  }
  document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
  if (handle && handle.state) {
    handle.state.armSafetyNet();
  }
}
