//! Appariement d'un nom de texte reçu d'un appelant avec les candidats du
//! catalogue. PUR : la récupération des candidats est ailleurs (`lj-store`),
//! ici on choisit — ou on refuse.
//!
//! Trois règles, par autorité décroissante, **chacune exigeant une cible
//! unique** : la clé de rapprochement, le numéro officiel de tête, le préfixe.
//! Deux candidats à égalité, c'est une question, pas un choix — un rattrapage
//! qui devine se trompe en silence, là où l'erreur corrective fait travailler
//! l'appelant.

use lj_fold::slug;

/// Règle qui a tranché — journalisée côté appelant, lue par le banc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Même clé de rapprochement : l'appelant a écrit le nom, à la ponctuation près.
    Cle,
    /// Même numéro officiel de tête (`loi n° 65-557`), quelle que soit la suite.
    Numero,
    /// La clé demandée préfixe celle du candidat : le catalogue tronque ses slugs.
    Prefixe,
}

/// Clé de rapprochement d'un nom de texte : le slug privé de ses tirets. Le
/// tiret ne porte rien — il sépare des mots —, et l'endroit où l'apostrophe le
/// fait tomber ne s'invente pas : le catalogue écrit
/// `code-de-la-construction-et-de-l-habitation`, un appelant écrit
/// `...-de-lhabitation`, c'est le même code.
pub fn match_key(name: &str) -> String {
    slug(name, '-').replace('-', "")
}

/// Natures d'instrument qui portent un numéro officiel. `code` en est absent :
/// un code n'est pas numéroté, et `code-2-…` ne doit pas se lire comme un numéro.
const NATURES_NUMEROTEES: &[&str] = &[
    "loi",
    "ordonnance",
    "decret",
    "arrete",
    "reglement",
    "directive",
];

/// Jetons qui séparent la nature du numéro sans rien porter : l'abréviation de
/// « numéro » et les sigles d'origine européenne.
const JETONS_AVANT_NUMERO: &[&str] = &["n", "no", "ue", "ce", "cee"];

/// Numéro officiel **de tête** d'un nom d'instrument (`loi n° 65-557 du 10
/// juillet 1965` → `65-557`), ou `None`.
///
/// De tête, c'est-à-dire porté par le texte lui-même et non cité dans son
/// intitulé : sans cette borne, `décret n° 2019-1507 portant modification du
/// décret n° 93-1362` répondrait aux deux numéros, et un texte modificatif
/// capterait les demandes visant le texte qu'il modifie.
pub fn head_number(name: &str) -> Option<String> {
    let s = slug(name, '-');
    let mut toks = s.split('-');
    if !NATURES_NUMEROTEES.contains(&toks.next()?) {
        return None;
    }
    let mut tok = toks.next()?;
    while JETONS_AVANT_NUMERO.contains(&tok) {
        tok = toks.next()?;
    }
    let annee = tok;
    if !matches!(annee.len(), 2 | 4) || !annee.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rang = toks.next()?;
    if rang.is_empty() || rang.len() > 5 || !rang.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{annee}-{rang}"))
}

/// Choisit le candidat que `demande` désigne parmi `candidats` — chacun donné
/// par son `(slug, titre)` de catalogue. Rend son index et la règle qui a
/// tranché, ou `None` si aucune règle ne désigne une cible **unique**.
pub fn pick(demande: &str, candidats: &[(&str, &str)]) -> Option<(usize, Rule)> {
    let unique = |mut hits: Vec<usize>, regle: Rule| match (hits.pop(), hits.pop()) {
        (Some(i), None) => Some((i, regle)),
        _ => None,
    };

    let cle = match_key(demande);
    let hits: Vec<usize> = candidats
        .iter()
        .enumerate()
        .filter(|(_, (s, t))| match_key(s) == cle || match_key(t) == cle)
        .map(|(i, _)| i)
        .collect();
    if let Some(hit) = unique(hits, Rule::Cle) {
        return Some(hit);
    }

    if let Some(num) = head_number(demande) {
        let hits: Vec<usize> = candidats
            .iter()
            .enumerate()
            .filter(|(_, (s, t))| {
                head_number(s).as_deref() == Some(&num) || head_number(t).as_deref() == Some(&num)
            })
            .map(|(i, _)| i)
            .collect();
        if let Some(hit) = unique(hits, Rule::Numero) {
            return Some(hit);
        }
    }

    let hits: Vec<usize> = candidats
        .iter()
        .enumerate()
        .filter(|(_, (s, t))| match_key(s).starts_with(&cle) || match_key(t).starts_with(&cle))
        .map(|(i, _)| i)
        .collect();
    unique(hits, Rule::Prefixe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cle_ignore_la_ponctuation_du_slug() {
        assert_eq!(
            match_key("code-general-des-impots"),
            match_key("Code général des impôts")
        );
        assert_eq!(
            match_key("code-de-la-construction-et-de-lhabitation"),
            match_key("code-de-la-construction-et-de-l-habitation")
        );
        assert_eq!(
            match_key("code-de-lentree-et-du-sejour-des-etrangers-et-du-droit-dasile"),
            match_key("code-de-l-entree-et-du-sejour-des-etrangers-et-du-droit-d-asile")
        );
        assert_eq!(
            match_key("code-des-procedures-civiles-dexecution"),
            match_key("code-des-procedures-civiles-d-execution")
        );
        assert_eq!(
            match_key("code-rural-et-de-la-peche-maritime"),
            match_key("Code rural et de la pêche maritime")
        );
        // Connecteurs absents : le texte visé n'est pas nommé, la question reste.
        assert_ne!(
            match_key("code-entree-sejour-etrangers-droit-asile"),
            match_key("code-de-l-entree-et-du-sejour-des-etrangers-et-du-droit-d-asile")
        );
        // Deux codes voisins ne se confondent pas.
        assert_ne!(
            match_key("code-penal"),
            match_key("code-de-procedure-penale")
        );
    }

    /// A text's free name must fall within the alphabet of `legal_text.slug`, the
    /// one `/norm/{juris}/{slug}` serves.
    #[test]
    fn la_cle_part_de_l_alphabet_du_catalogue() {
        assert_eq!(slug("Code civil", '-'), "code-civil");
        assert_eq!(
            slug("Code de procédure civile", '-'),
            "code-de-procedure-civile"
        );
        assert_eq!(
            slug(
                "Code de l'entrée et du séjour des étrangers et du droit d'asile",
                '-'
            ),
            "code-de-l-entree-et-du-sejour-des-etrangers-et-du-droit-d-asile"
        );
        assert_eq!(slug("code-civil", '-'), "code-civil");
    }

    #[test]
    fn numero_de_tete_seulement() {
        assert_eq!(
            head_number("loi-n-65-557-du-10-juillet-1965-fixant-le-statut"),
            Some("65-557".into())
        );
        assert_eq!(
            head_number("loi-65-557-du-10-juillet-1965"),
            Some("65-557".into())
        );
        assert_eq!(
            head_number("Loi n° 78-17 du 6 janvier 1978 relative à l'informatique"),
            Some("78-17".into())
        );
        assert_eq!(
            head_number("reglement-ue-2024-1689-du-parlement-europeen"),
            Some("2024-1689".into())
        );
        // Le numéro cité dans l'intitulé d'un modificatif n'est pas le sien.
        assert_eq!(
            head_number(
                "decret-n-2019-1507-du-30-decembre-2019-portant-modification-du-decret-n-93-1362"
            ),
            Some("2019-1507".into())
        );
        // Pas de numéro : une date n'en est pas un, un code n'en porte pas.
        assert_eq!(head_number("loi-du-6-juillet-1989"), None);
        assert_eq!(head_number("arrete-du-6-juillet-1989"), None);
        assert_eq!(head_number("code-general-des-impots"), None);
        assert_eq!(head_number("loi-de-finances-pour-1994-n-93-1352"), None);
    }

    #[test]
    fn le_prefixe_rattrape_un_slug_catalogue_tronque() {
        // The catalogue truncates its slugs: a caller writing the full name
        // lands on a prefix.
        let candidats = [
            (
                "decret-n-93-1371-du-30-decembre-1993-modifiant-le-decret-93451",
                "Décret n°93-1371",
            ),
            (
                "decret-n-2019-1507-du-30-decembre-2019-portant-modification-du-decret-n-93-1362",
                "Décret n° 2019-1507",
            ),
            (
                "decret-n-93-1362-du-30-decembre-1993-relatif-a-la-manifestation-de-volonte-aux",
                "Décret n°93-1362 du 30 décembre 1993 relatif aux déclarations de nationalité",
            ),
        ];
        let (i, regle) = pick("decret-n-93-1362-du-30-decembre-1993", &candidats).unwrap();
        assert_eq!(i, 2);
        // Le numéro tranche avant le préfixe : les deux visent la même ligne.
        assert_eq!(regle, Rule::Numero);
    }

    #[test]
    fn le_numero_tranche_ce_que_la_formulation_brouille() {
        // Name reworded, number right.
        let candidats = [
            (
                "decret-n-2019-536-du-29-mai-2019-pris-pour-l-application-de-la-loi-n-78-17-du-6",
                "Décret n° 2019-536",
            ),
            (
                "loi-n-78-17-du-6-janvier-1978-relative-a-l-informatique-aux-fichiers-et-aux",
                "Loi n° 78-17 du 6 janvier 1978 relative à l'informatique, aux fichiers et aux libertés",
            ),
        ];
        let hit = pick(
            "loi-n-78-17-du-6-janvier-1978-informatique-et-libertes",
            &candidats,
        );
        assert_eq!(hit, Some((1, Rule::Numero)));
    }

    #[test]
    fn deux_cibles_ne_se_tranchent_pas() {
        // « loi du 6 juillet 1989 » désigne 89-456, 89-461 et 89-462 : c'est
        // l'usage qui choisit, donc la table d'alias, jamais l'appariement.
        let candidats = [
            (
                "loi-n-89-461-du-6-juillet-1989-modifiant-le-code-de-procedure-penale",
                "Loi n° 89-461",
            ),
            (
                "loi-n-89-462-du-6-juillet-1989-tendant-a-ameliorer-les-rapports-locatifs-et",
                "Loi n° 89-462",
            ),
        ];
        assert_eq!(pick("loi-du-6-juillet-1989", &candidats), None);
    }

    #[test]
    fn la_cle_gagne_sur_un_titre() {
        let candidats = [
            (
                "procedure-penale-du-livre-ii-du-code-rural-et-de-la-peche-maritime",
                "Procédure pénale du livre II du code rural et de la pêche maritime",
            ),
            ("code-rural", "Code rural et de la pêche maritime"),
        ];
        assert_eq!(
            pick("code-rural-et-de-la-peche-maritime", &candidats),
            Some((1, Rule::Cle))
        );
    }
}
