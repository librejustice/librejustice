// Geometrie de la selection de texte (port de l'effet `selectionchange` /
// `scroll`(capture) / `resize` de `decision-body.tsx`). `web-sys` n'expose pas
// `Selection`/`Range`/`DomRect` sans feature dans le crate fige : on lit la
// selection en JS et on rappelle `onChange(text, top, left)` cote Rust.
//
// `onChange(null, 0, 0)` quand la selection est vide/collapsed/hors de
// `articleEl`, ou que son rect est degenere — Rust traite `null` comme None.
// Renvoie une fn de deconnexion (cleanup des 3 listeners).
export function observeSelection(articleEl, onChange) {
  if (typeof window === "undefined" || articleEl === null) {
    return () => {};
  }

  const update = () => {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0 || selection.isCollapsed) {
      onChange(null, 0, 0);
      return;
    }

    const range = selection.getRangeAt(0);
    const anchor =
      range.commonAncestorContainer.nodeType === Node.ELEMENT_NODE
        ? range.commonAncestorContainer
        : range.commonAncestorContainer.parentElement;

    if (!anchor || !articleEl.contains(anchor)) {
      onChange(null, 0, 0);
      return;
    }

    const rect = range.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) {
      onChange(null, 0, 0);
      return;
    }

    onChange(
      selection.toString(),
      Math.max(16, rect.top - 12),
      rect.left + rect.width / 2,
    );
  };

  document.addEventListener("selectionchange", update);
  window.addEventListener("scroll", update, true);
  window.addEventListener("resize", update);

  return () => {
    document.removeEventListener("selectionchange", update);
    window.removeEventListener("scroll", update, true);
    window.removeEventListener("resize", update);
  };
}

// Vide la selection courante (port de `removeAllRanges`).
export function clearSelection() {
  window.getSelection()?.removeAllRanges();
}
