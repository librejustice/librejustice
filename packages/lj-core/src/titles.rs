//! Canonical composition of a decision title, shared by the API (display on
//! read) and the ingest (writing `search_title`): one function, one format. The
//! seat is not part of the second: `compose_search_title` passes `None`, so
//! recomposing it stales no indexed title.
//!
//! Format: `<juridiction>[, <siège>], <date FR>[, <nom d'usage>],
//! <premier numéro>`, e.g.
//! « Cour d'appel de Paris, pôle 5, 3e chambre · formation à trois,
//! 25 février 2026, 21/04532 ». The seat is recomposed from the structured
//! seat (`seat`) and the `formation:*` / `office:*` labels, never from a
//! source string.

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
/// `facet_value` ; la table Rust sert l'écriture ingest (`search_title`) sans
/// aller-retour DB.
pub const FORMATION_LABELS: &[(&str, &str)] = &[
    ("formation:A_TROIS", "Formation à trois"),
    ("formation:A_CINQ", "Formation à cinq"),
    ("formation:JUGE_UNIQUE", "Juge unique"),
    ("formation:CHAMBRE_SEULE", "Chambre jugeant seule"),
    ("formation:RESTREINTE", "Formation restreinte"),
    ("formation:SECTION", "Formation de section"),
    ("formation:PLENIERE", "Formation plénière"),
];

/// Labels des offices `office:*` — miroir du référentiel.
pub const OFFICE_LABELS: &[(&str, &str)] = &[
    ("office:JLD", "Juge des libertés et de la détention"),
    ("office:JAF", "Juge aux affaires familiales"),
    ("office:JCP", "Juge des contentieux de la protection"),
    ("office:JEX", "Juge de l'exécution"),
    ("office:JUGE_ENFANTS", "Juge des enfants"),
    ("office:PREMIER_PRESIDENT", "Premier président"),
    ("office:MAGISTRAT_DESIGNE", "Magistrat désigné"),
    ("office:JUGE_EXPROPRIATION", "Juge de l'expropriation"),
    ("office:MISE_EN_ETAT", "Juge de la mise en état"),
];

/// Labels des types de juridiction — miroir du seed `facet_value` ; repli du
/// titre quand la décision ne porte pas de nom de juridiction extrait (CEDH,
/// CJUE, CONSTIT, TC…).
pub const JURISDICTION_TYPE_LABELS: &[(&str, &str)] = &[
    ("CE", "Conseil d'État"),
    ("CAA", "Cour administrative d'appel"),
    ("TA", "Tribunal administratif"),
    ("CC", "Cour de cassation"),
    ("CA", "Cour d'appel"),
    ("TJ", "Tribunal judiciaire"),
    ("TCOM", "Tribunal de commerce"),
    ("CPH", "Conseil de prud'hommes"),
    ("CNDA", "Cour nationale du droit d'asile"),
    ("CONSTIT", "Conseil constitutionnel"),
    ("TC", "Tribunal des conflits"),
    ("CEDH", "Cour européenne des droits de l'homme"),
    ("CJUE", "Cour de justice de l'Union européenne"),
    ("DDD", "Défenseur des droits"),
    ("ADLC", "Autorité de la concurrence"),
    ("COMEU", "Commission européenne"),
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

/// Le type de formation est-il déjà dit par la position ? Sous-ensemble de
/// tokens pliés, mots génériques exclus — « Sous-sections réunies » ⊆
/// « Sous-sections 2/6 réunies », « Formation plénière » ⊆ « Assemblée
/// plénière », mais « Formation à trois » ⊄ « 3e chambre ».
fn formation_redundant(base: &str, formation: &str) -> bool {
    let base_folded = lj_fold::fold_chars(base);
    let base_tokens: Vec<&str> = base_folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    lj_fold::fold_chars(formation)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !matches!(*t, "formation" | "de" | "a" | "en" | "du"))
        .all(|t| base_tokens.contains(&t))
}

/// Chamber, then formation, then office, joined by a middle dot; a missing axis
/// leaves no hole. Folding drops what would be said twice, and « Juge unique »
/// beside an office is the corpus rather than an edge case: nearly every decision
/// carrying both axes has it, so without that filter the seat reads « juge unique
/// · magistrat désigné ».
pub fn seat_display(
    jurisdiction: &str,
    chamber_label: Option<&str>,
    formation_label: Option<&str>,
    office_label: Option<&str>,
) -> Option<String> {
    let jur_folded = lj_fold::fold_chars(jurisdiction);
    let base = chamber_label
        .map(str::trim)
        .filter(|p| !p.is_empty() && !jur_folded.contains(&lj_fold::fold_chars(p)));
    let office = office_label
        .map(str::trim)
        .filter(|o| !o.is_empty() && !jur_folded.contains(&lj_fold::fold_chars(o)));
    let dit_par_l_office = |f: &str| office.is_some() && lj_fold::fold_chars(f) == "juge unique";
    let formation = formation_label.map(str::trim).filter(|f| {
        !f.is_empty()
            && !jur_folded.contains(&lj_fold::fold_chars(f))
            && !base.is_some_and(|b| formation_redundant(b, f))
            && !dit_par_l_office(f)
    });
    let qualifiers: Vec<&str> = formation.into_iter().chain(office).collect();
    let Some(b) = base else {
        return match qualifiers.is_empty() {
            true => None,
            false => Some(
                qualifiers
                    .into_iter()
                    .map(mid_title)
                    .collect::<Vec<_>>()
                    .join(" · "),
            ),
        };
    };
    // « Chambre jugeant seule » grafts onto a position already ending in the
    // chamber it qualifies, so it is never a segment of its own.
    if let [q] = qualifiers[..] {
        if (lj_fold::fold_chars(b).ends_with("chambre")
            || lj_fold::fold_chars(b).ends_with("sous-section"))
            && q.get(..8)
                .is_some_and(|p| p.eq_ignore_ascii_case("chambre "))
        {
            return Some(format!("{} {}", mid_title(b), &q[8..]));
        }
    }
    let mut segments = vec![mid_title(b)];
    segments.extend(qualifiers.into_iter().map(mid_title));
    Some(segments.join(" · "))
}

/// The number the title names the decision by, which is the head one: it is
/// under that number that a decision is cited. The others are what reaches it,
/// not what names it, and no title could hold them all, a mass Cour de cassation
/// judgment carrying pourvois by the hundred. The number a search bit on is said by
/// the result, next to the title, never inside it.
pub fn docket_display(numbers: &[String]) -> Option<String> {
    numbers
        .iter()
        .map(|n| n.trim())
        .find(|n| !n.is_empty())
        .map(str::to_string)
}

/// Canonical title `<court>[, <seat>], <FR date>[, <case name>], <numbers>`.
/// The court is the referential label the caller already resolved, the seat
/// comes from [`seat_display`], and the case name precedes the numbers the way
/// the Conseil d'Etat cites its own decisions.
pub fn decision_title(
    jurisdiction: &str,
    seat: Option<&str>,
    date_lecture: Option<&str>,
    case_name: Option<&str>,
    docket: Option<&str>,
) -> String {
    let mut parts = vec![jurisdiction.trim().to_string()];
    if let Some(s) = seat.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(s.to_string());
    }
    if let Some(date) = date_lecture.filter(|d| !d.is_empty()) {
        parts.push(format_fr_date(date));
    }
    if let Some(nom) = case_name.map(str::trim).filter(|n| !n.is_empty()) {
        parts.push(nom.to_string());
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
            Some("Pôle 5, 3e chambre"),
            Some("Formation à trois"),
            None,
        );
        assert_eq!(
            seat.as_deref(),
            Some("pôle 5, 3e chambre · formation à trois")
        );
    }

    #[test]
    fn seat_cc_chambre_deja_dans_la_juridiction() {
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

    /// « Juge unique » does not survive an office, which already says the
    /// magistrate sits alone, chamber or not.
    #[test]
    fn juge_unique_tombe_devant_un_office() {
        assert_eq!(
            seat_display(
                "Tribunal judiciaire de Lyon",
                None,
                Some("Juge unique"),
                Some("Juge des référés"),
            )
            .as_deref(),
            Some("juge des référés")
        );
        assert_eq!(
            seat_display(
                "Tribunal administratif de Melun",
                Some("3e chambre"),
                Some("Juge unique"),
                Some("Magistrat désigné"),
            )
            .as_deref(),
            Some("3e chambre · magistrat désigné")
        );
    }

    /// The three axes hold the line when none repeats another.
    #[test]
    fn les_trois_axes_se_suivent_dans_l_ordre() {
        assert_eq!(
            seat_display(
                "Cour administrative d'appel de Nantes",
                Some("5e chambre"),
                Some("Formation à trois"),
                Some("Magistrat désigné"),
            )
            .as_deref(),
            Some("5e chambre · formation à trois · magistrat désigné")
        );
        assert_eq!(
            seat_display(
                "Cour de cassation",
                None,
                Some("Formation restreinte"),
                Some("Premier président"),
            )
            .as_deref(),
            Some("formation restreinte · premier président")
        );
    }

    #[test]
    fn seat_vide_sans_axe() {
        assert_eq!(
            seat_display("Tribunal de commerce de Lyon", None, None, None),
            None
        );
    }

    #[test]
    fn le_nom_dusage_precede_le_numero() {
        let title = decision_title(
            "Conseil d'État",
            Some("formation de section"),
            Some("2011-12-09"),
            Some("Marcou"),
            Some("337255"),
        );
        assert_eq!(
            title,
            "Conseil d'État, formation de section, 9 décembre 2011, Marcou, 337255"
        );
    }

    #[test]
    fn titre_complet() {
        let title = decision_title(
            "Cour d'appel de Paris",
            Some("pôle 5, 3e chambre · formation à trois"),
            Some("2026-02-25"),
            None,
            Some("21/04532"),
        );
        assert_eq!(
            title,
            "Cour d'appel de Paris, pôle 5, 3e chambre · formation à trois, 25 février 2026, 21/04532"
        );
    }

    #[test]
    fn le_titre_ne_porte_que_le_numero_de_tete() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert_eq!(docket_display(&[]), None);
        assert_eq!(docket_display(&n(&["  ", ""])), None);
        assert_eq!(docket_display(&n(&["490357"])).as_deref(), Some("490357"));
        assert_eq!(
            docket_display(&n(&["  ", "490595", "490625"])).as_deref(),
            Some("490595")
        );
        assert_eq!(
            decision_title(
                "Conseil d'État",
                None,
                Some("2024-10-01"),
                None,
                docket_display(&n(&["490357", "490595", "490625"])).as_deref(),
            ),
            "Conseil d'État, 1 octobre 2024, 490357"
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
