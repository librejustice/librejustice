//! A link that leaves the route table must leave the router.
//!
//! `leptos_router` intercepts every click on a same-origin anchor unless it
//! carries `download` or `rel="external"` (`location::handle_anchor_click`). An
//! anchor to a data surface without either goes into client navigation, no
//! route matches, and the user gets the 404 page, cleared only on a refresh.
//!
//! This harness reads the crate's sources rather than the rendered DOM: the
//! defect lives in the attribute, and a render enough to observe it would need
//! a browser, which is the journey smoke's job rather than a `cargo test`'s.

use std::path::{Path, PathBuf};

/// Ancre ouvrante repérée dans une source, du `<a` au `>` qui la ferme.
struct Ancre {
    fichier: PathBuf,
    ligne: usize,
    balise: String,
}

fn sources(racine: &Path, dans: &mut Vec<PathBuf>) {
    for entree in std::fs::read_dir(racine).expect("lecture du dossier de sources") {
        let chemin = entree.expect("entrée de dossier").path();
        if chemin.is_dir() {
            sources(&chemin, dans);
        } else if chemin.extension().is_some_and(|e| e == "rs") {
            dans.push(chemin);
        }
    }
}

/// Balises ouvrantes `<a …>` du fichier. Le `>` est cherché hors chaîne : une
/// classe Tailwind en porte (`[&>svg]:…`).
fn ancres(fichier: &Path, source: &str) -> Vec<Ancre> {
    let octets = source.as_bytes();
    let mut trouvees = Vec::new();
    let mut i = 0;
    while let Some(pos) = source[i..].find("<a") {
        let debut = i + pos;
        i = debut + 2;
        // `<article`, `<abbr` : la balise s'arrête au nom.
        if !octets
            .get(debut + 2)
            .is_some_and(|c| c.is_ascii_whitespace())
        {
            continue;
        }
        let mut dans_chaine = false;
        let mut fin = None;
        for (offset, c) in source[debut..].char_indices() {
            match c {
                '"' => dans_chaine = !dans_chaine,
                '>' if !dans_chaine => {
                    fin = Some(debut + offset);
                    break;
                }
                _ => {}
            }
        }
        let Some(fin) = fin else { continue };
        trouvees.push(Ancre {
            fichier: fichier.to_path_buf(),
            ligne: source[..debut].lines().count(),
            balise: source[debut..=fin].to_string(),
        });
        i = fin;
    }
    trouvees
}

/// Valeur de `href=`, telle qu'écrite : `"littéral"` ou nom d'une liaison.
fn href(balise: &str) -> Option<&str> {
    let apres = &balise[balise.find("href=")? + "href=".len()..];
    let fin = apres
        .find(|c: char| c.is_ascii_whitespace() || c == '>')
        .unwrap_or(apres.len());
    Some(apres[..fin].trim_matches('"'))
}

/// Cible visée par l'ancre. Un `href` littéral se lit tel quel ; un `href=nom`
/// se résout sur la liaison du même fichier, dont on prend le corps.
fn cible(source: &str, valeur: &str) -> String {
    if valeur.starts_with('/') {
        return valeur.to_string();
    }
    let Some(pos) = source.find(&format!("let {valeur} =")) else {
        return String::new();
    };
    source[pos..].chars().take(600).collect()
}

fn sort_du_routeur(balise: &str) -> bool {
    if balise.contains("download") {
        return true;
    }
    let Some(pos) = balise.find("rel=") else {
        return false;
    };
    let apres = &balise[pos + "rel=".len()..];
    let Some(ouvrante) = apres.find('"') else {
        return false;
    };
    let reste = &apres[ouvrante + 1..];
    let Some(fermante) = reste.find('"') else {
        return false;
    };
    reste[..fermante]
        .split_whitespace()
        .any(|r| r == "external")
}

/// Les deux préfixes servis par Axum, hors table de routes du front.
const SURFACES: [&str; 2] = ["/api/", "/internal/"];

#[test]
fn une_ancre_vers_l_api_sort_du_routeur() {
    let racine = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut fichiers = Vec::new();
    sources(&racine, &mut fichiers);

    let mut vues = 0usize;
    let mut vers_api = Vec::new();
    let mut fautives = Vec::new();

    for fichier in &fichiers {
        let source = std::fs::read_to_string(fichier).expect("lecture d'une source");
        for ancre in ancres(fichier, &source) {
            vues += 1;
            let Some(valeur) = href(&ancre.balise) else {
                continue;
            };
            let cible = cible(&source, valeur);
            if !SURFACES.iter().any(|s| cible.contains(s)) {
                continue;
            }
            let ou = format!(
                "{}:{}",
                ancre.fichier.strip_prefix(&racine).unwrap().display(),
                ancre.ligne
            );
            vers_api.push(ou.clone());
            if !sort_du_routeur(&ancre.balise) {
                fautives.push(ou);
            }
        }
    }

    // Un garde qui ne voit plus rien passerait pour vert : on vérifie d'abord
    // que le scan mord encore sur le front tel qu'il est écrit.
    assert!(
        vues >= 20,
        "{vues} ancres trouvées dans {} fichiers : le scan ne lit plus les vues",
        fichiers.len()
    );
    assert!(
        !vers_api.is_empty(),
        "aucune ancre vers {SURFACES:?} trouvée : la résolution du href ne mord plus"
    );
    assert!(
        fautives.is_empty(),
        "ancres vers {SURFACES:?} que le routeur va intercepter (ajouter \
         rel=\"external\" ou download) : {fautives:?}"
    );
}
