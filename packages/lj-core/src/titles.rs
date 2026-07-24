//! Composition canonique du titre de décision (ADR 0049 / 0170) — partagée
//! par l'API (affichage à la lecture) et l'ingest (écriture de
//! `search_title`) : une seule fonction, un seul format.
//!
//! Format : `<juridiction>[, <siège>], <date FR>, <premier numéro>` — p. ex.
//! « Cour d'appel de Paris, pôle 5 — 3e chambre · formation à trois,
//! 25 février 2026, 21/04532 ». Le siège est recomposé depuis les axes
//! structurés (`chamber_position` + labels `formation:*` / `office:*`),
//! jamais depuis une chaîne source.

const FR_MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// « 2026-05-29 » → « 29 mai 2026 ». Passe-plat si pas une date ISO complète.
pub fn format_fr_date(date_lecture: &str) -> String {
    let parts: Vec<&str> = date_lecture.split('-').collect();
    if parts.len() != 3 {
        return date_lecture.to_string();
    }
    match (
        parts[0].parse::<i32>(),
        parts[1].parse::<usize>(),
        parts[2].parse::<i32>(),
    ) {
        (Ok(year), Ok(month), Ok(day)) if (1..=12).contains(&month) => {
            format!("{} {} {}", day, FR_MONTHS[month - 1], year)
        }
        _ => date_lecture.to_string(),
    }
}

/// Labels des types de formation `formation:*` — miroir exact du seed
/// `facet_value` (migration 0117) ; la table Rust sert l'écriture ingest
/// (`search_title`) sans aller-retour DB.
pub const FORMATION_LABELS: &[(&str, &str)] = &[
    ("formation:A_TROIS", "Formation à trois"),
    ("formation:A_CINQ", "Formation à cinq"),
    ("formation:JUGE_UNIQUE", "Juge unique"),
    ("formation:CHAMBRE_SEULE", "Chambre jugeant seule"),
    ("formation:RESTREINTE", "Formation restreinte"),
    ("formation:SECTION", "Formation de section"),
    ("formation:PLENIERE", "Formation plénière"),
    ("formation:MIXTE", "Formation mixte"),
    ("formation:SSR", "Sous-sections réunies"),
    ("formation:CHAMBRES_REUNIES", "Chambres réunies"),
    ("formation:ASSEMBLEE", "Assemblée du contentieux"),
    ("formation:SPECIALISEE", "Formation spécialisée"),
];

/// Labels des offices `office:*` — miroir du référentiel (seeds 0107/0117).
pub const OFFICE_LABELS: &[(&str, &str)] = &[
    ("office:JLD", "Juge des libertés et de la détention"),
    ("office:JAF", "Juge aux affaires familiales"),
    ("office:JCP", "Juge des contentieux de la protection"),
    ("office:JEX", "Juge de l'exécution"),
    ("office:JUGE_ENFANTS", "Juge des enfants"),
    ("office:PREMIER_PRESIDENT", "Premier président"),
    ("office:MAGISTRAT_DESIGNE", "Magistrat désigné"),
    ("office:JUGE_REFERES", "Juge des référés"),
    (
        "office:PRESIDENT_SECTION_CONTENTIEUX",
        "Président de la section du contentieux",
    ),
    ("office:JUGE_EXPROPRIATION", "Juge de l'expropriation"),
];

/// Labels des types de juridiction — miroir du seed `facet_value`
/// (migration 0102) ; repli du titre quand la décision ne porte pas de nom de
/// juridiction extrait (CEDH, CJUE, CONSTIT, TC…).
pub const JURISDICTION_TYPE_LABELS: &[(&str, &str)] = &[
    ("CE", "Conseil d'État"),
    ("CAA", "Cour administrative d'appel"),
    ("TA", "Tribunal administratif"),
    ("CC", "Cour de cassation"),
    ("CA", "Cour d'appel"),
    ("TJ", "Tribunal judiciaire"),
    ("TCOM", "Tribunal de commerce"),
    ("CNDA", "Cour nationale du droit d'asile"),
    ("CONSTIT", "Conseil constitutionnel"),
    ("TC", "Tribunal des conflits"),
    ("CEDH", "Cour européenne des droits de l'homme"),
    ("CJUE", "Cour de justice de l'Union européenne"),
];

pub fn jurisdiction_type_label(code: &str) -> Option<&'static str> {
    JURISDICTION_TYPE_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, l)| *l)
}

pub fn formation_label(uid: &str) -> Option<&'static str> {
    FORMATION_LABELS
        .iter()
        .find(|(u, _)| *u == uid)
        .map(|(_, l)| *l)
}

pub fn office_label(uid: &str) -> Option<&'static str> {
    OFFICE_LABELS
        .iter()
        .find(|(u, _)| *u == uid)
        .map(|(_, l)| *l)
}

/// Minuscule mi-titre : première lettre abaissée sauf sigle (2ᵉ caractère déjà
/// majuscule). « Pôle 5 » → « pôle 5 », « Chambre B » → « chambre B »,
/// « DALO » intact.
fn mid_title(s: &str) -> String {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(first), second)
            if first.is_uppercase() && !second.is_some_and(|c| c.is_uppercase()) =>
        {
            first.to_lowercase().collect::<String>() + &s[first.len_utf8()..]
        }
        _ => s.to_string(),
    }
}

/// Comparaison de redondance : minuscules + accents aplatis (sous-ensemble du
/// pliage lj-extract, suffisant pour des labels référentiels).
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'À' | 'Â' | 'Ä' | 'à' | 'â' | 'ä' => vec!['a'],
            'É' | 'È' | 'Ê' | 'Ë' | 'é' | 'è' | 'ê' | 'ë' => vec!['e'],
            'Î' | 'Ï' | 'î' | 'ï' => vec!['i'],
            'Ô' | 'Ö' | 'ô' | 'ö' => vec!['o'],
            'Ù' | 'Û' | 'Ü' | 'ù' | 'û' | 'ü' => vec!['u'],
            'Ç' | 'ç' => vec!['c'],
            _ => c.to_lowercase().collect(),
        })
        .collect()
}

/// Le type de formation est-il déjà dit par la position ? Sous-ensemble de
/// tokens pliés, mots génériques exclus — « Sous-sections réunies » ⊆
/// « Sous-sections 2/6 réunies », « Formation plénière » ⊆ « Assemblée
/// plénière », mais « Formation à trois » ⊄ « 3e chambre ».
fn formation_redundant(base: &str, formation: &str) -> bool {
    let base_folded = fold(base);
    let base_tokens: Vec<&str> = base_folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    fold(formation)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !matches!(*t, "formation" | "de" | "a" | "en" | "du"))
        .all(|t| base_tokens.contains(&t))
}

/// Siège recomposé depuis les axes structurés (ADR 0170) :
/// `chamber_position`, qualifié après un point médian par le type de formation
/// ou à défaut l'office (« 7e chambre · juge des libertés et de la détention »)
/// — le « · » (et non des parenthèses) évite les parenthèses imbriquées quand le
/// titre est lui-même cité entre parenthèses ; sans position, formation puis
/// office. Les redondances tombent par
/// comparaison pliée — la chambre déjà portée par le label de juridiction
/// (« Cour de cassation, deuxième chambre civile ») n'est pas répétée, une
/// position « Sous-sections 2/6 réunies » n'est pas suffixée
/// « (sous-sections réunies) ».
pub fn seat_display(
    jurisdiction: &str,
    chamber_position: Option<&str>,
    formation_label: Option<&str>,
    office_label: Option<&str>,
) -> Option<String> {
    let jur_folded = fold(jurisdiction);
    let base = chamber_position
        .map(str::trim)
        .filter(|p| !p.is_empty() && !jur_folded.contains(&fold(p)));
    let formation = formation_label.map(str::trim).filter(|f| {
        !f.is_empty()
            && !jur_folded.contains(&fold(f))
            && !base.is_some_and(|b| formation_redundant(b, f))
    });
    let office = office_label
        .map(str::trim)
        .filter(|o| !o.is_empty() && !jur_folded.contains(&fold(o)));
    match base {
        Some(b) => match formation.or(office) {
            // « 5e chambre » + « Chambre jugeant seule » → « 5e chambre
            // jugeant seule » (idem « 3e sous-section jugeant seule ») : le
            // qualificatif en « chambre … » se greffe à une position qui
            // finit déjà par la chambre / sous-section qu'il qualifie.
            Some(q)
                if (fold(b).ends_with("chambre") || fold(b).ends_with("sous-section"))
                    && q.get(..8)
                        .is_some_and(|p| p.eq_ignore_ascii_case("chambre ")) =>
            {
                Some(format!("{} {}", mid_title(b), &q[8..]))
            }
            Some(q) => Some(format!("{} · {}", mid_title(b), mid_title(q))),
            None => Some(mid_title(b)),
        },
        // Sans position, le rôle est plus parlant que le type de formation :
        // « juge des référés », pas « juge unique ».
        None => office.or(formation).map(mid_title),
    }
}

/// Titre canonique « <juridiction>[, <siège>], <date FR>, <numéro> ». La
/// juridiction est le label référentiel déjà résolu par l'appelant ; le siège
/// sort de [`seat_display`].
pub fn decision_title(
    jurisdiction: &str,
    seat: Option<&str>,
    date_lecture: Option<&str>,
    docket: Option<&str>,
) -> String {
    let mut parts = vec![jurisdiction.trim().to_string()];
    if let Some(s) = seat.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(s.to_string());
    }
    if let Some(date) = date_lecture.filter(|d| !d.is_empty()) {
        parts.push(format_fr_date(date));
    }
    if let Some(n) = docket.map(str::trim).filter(|n| !n.is_empty()) {
        parts.push(n.to_string());
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seat_pole_chambre_avec_formation() {
        let seat = seat_display(
            "Cour d'appel de Paris",
            Some("Pôle 5 — 3e chambre"),
            Some("Formation à trois"),
            None,
        );
        assert_eq!(
            seat.as_deref(),
            Some("pôle 5 — 3e chambre · formation à trois")
        );
    }

    #[test]
    fn seat_cc_chambre_deja_dans_la_juridiction() {
        // Le label référentiel cass_civ2 porte déjà la chambre.
        let seat = seat_display(
            "Cour de cassation, deuxième chambre civile",
            Some("Deuxième chambre civile"),
            Some("Formation restreinte"),
            None,
        );
        assert_eq!(seat.as_deref(), Some("formation restreinte"));
    }

    #[test]
    fn seat_ssr_sans_suffixe_redondant() {
        let seat = seat_display(
            "Conseil d'État",
            Some("Sous-sections 2/6 réunies"),
            Some("Sous-sections réunies"),
            None,
        );
        assert_eq!(seat.as_deref(), Some("sous-sections 2/6 réunies"));
    }

    #[test]
    fn seat_chambre_jugeant_seule_greffee() {
        let seat = seat_display(
            "Cour administrative d'appel de Lyon",
            Some("5e chambre"),
            Some("Chambre jugeant seule"),
            None,
        );
        assert_eq!(seat.as_deref(), Some("5e chambre jugeant seule"));
        let seat = seat_display(
            "Conseil d'État",
            Some("3e sous-section"),
            Some("Chambre jugeant seule"),
            None,
        );
        assert_eq!(seat.as_deref(), Some("3e sous-section jugeant seule"));
    }

    #[test]
    fn seat_position_qualifiee_par_office() {
        let seat = seat_display(
            "Tribunal judiciaire de Nanterre",
            Some("7e chambre"),
            None,
            Some("Juge des libertés et de la détention"),
        );
        assert_eq!(
            seat.as_deref(),
            Some("7e chambre · juge des libertés et de la détention")
        );
    }

    #[test]
    fn seat_office_seul() {
        let seat = seat_display(
            "Tribunal judiciaire de Paris",
            None,
            None,
            Some("Juge des libertés et de la détention"),
        );
        assert_eq!(
            seat.as_deref(),
            Some("juge des libertés et de la détention")
        );
    }

    #[test]
    fn seat_office_prime_sans_position() {
        // « Chamb. référés (sup 10 000) » : office juge des référés + régime
        // juge unique — le rôle fait le siège.
        let seat = seat_display(
            "Tribunal judiciaire de Lyon",
            None,
            Some("Juge unique"),
            Some("Juge des référés"),
        );
        assert_eq!(seat.as_deref(), Some("juge des référés"));
    }

    #[test]
    fn seat_vide_sans_axe() {
        assert_eq!(
            seat_display("Tribunal de commerce de Lyon", None, None, None),
            None
        );
    }

    #[test]
    fn titre_complet() {
        let title = decision_title(
            "Cour d'appel de Paris",
            Some("pôle 5 — 3e chambre · formation à trois"),
            Some("2026-02-25"),
            Some("21/04532"),
        );
        assert_eq!(
            title,
            "Cour d'appel de Paris, pôle 5 — 3e chambre · formation à trois, 25 février 2026, 21/04532"
        );
    }

    #[test]
    fn mid_title_preserve_les_sigles() {
        assert_eq!(mid_title("Chambre B"), "chambre B");
        assert_eq!(mid_title("DALO"), "DALO");
        assert_eq!(
            mid_title("Étrangers et rétention"),
            "étrangers et rétention"
        );
    }
}
