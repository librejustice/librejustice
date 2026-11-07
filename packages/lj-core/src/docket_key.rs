//! Clé canonique d'un numéro de décision — pourvoi, rôle, requête.
//!
//! Un numéro réduit à ses alphanumériques minuscules. `19-20.312`, `19-20312`,
//! `1920312` et `19 20 312` donnent tous `1920312` : c'est la seule façon de
//! réunir les graphies, un tokenizer ne sachant que couper.
//!
//! Le pendant côté base est la fonction SQL `lj_docket_keys` (migration 0211),
//! sur laquelle l'index d'expression est bâti. Les deux implémentations sont
//! asservies au même oracle (`docket_keys.json`) : elles ne peuvent pas diverger
//! sans faire rougir le build.

use std::sync::LazyLock;

use regex::Regex;

/// Clé canonique d'un numéro (voir doc de module). Vide si le numéro ne porte
/// aucun alphanumérique.
pub fn docket_key(num: &str) -> String {
    num.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

macro_rules! slash {
    () => {
        r"\s*/\s*"
    };
}

/// A slash as the fonds writes it, spaces allowed on either side; a spaced dash
/// or dot, rarer, is left glued.
pub(crate) const SLASH: &str = slash!();

/// A pourvoi as the decisions write it behind its mark: `11-25536`, `06-41. 614`,
/// `02-14,799`, `09-72-219`, `0430.583`, `08-215.47`.
pub const POURVOI: &str = r"(?:\d{2}[-.]\s?\d{2}[-.,]?\s?\d{3}|\d{4}\.\d{3}|\d{2}-\d{3}\.\d{2})";

/// A role number with its slash: `21/04532`, `09/ 497`.
pub const ROLE_SLASHED: &str = concat!(r"\d{1,4}", slash!(), r"\d{2,7}");

/// A role number behind its mark: slashed, `2015F00459`, `2023IP00245`,
/// `11-04-000227`, `06-06235`, `2023063685`, `2015 2815`.
pub const ROLE: &str = concat!(
    r"(?:\d{1,4}",
    slash!(),
    r"\d{2,7}|\d{4}(?:\s?[a-z]\s?|[a-z]{2})\d{3,6}|\d{2}-\d{2}-\d{3,6}|\d{2}-\d{2,6}|\d{8,10}|\d{4}\s\d{3,4})"
);

/// A decision number inside a query, every jurisdiction: `07-11.687`,
/// `19-23664`, `23/01001`, `31399/23`, `2202166`, `2022021570`, `21PA01295`,
/// `2005J00072`, `C-369/90`, `SAN-2022-019`, `2010-104 QPC`, an ECLI. Each
/// branch is named after the courts its graphy belongs to ([`courts_of`]).
///
/// Bare digits run from five to ten. A postcode cannot empty a search: a bare run
/// standing alone buys weight and not a mode ([`declares_reference_explicitly`]).
/// The Strasbourg shape, digits over a two-digit year, covers nearly every CEDH
/// number; its floor sits at three because two over two is a written date,
/// `12/03` inside `12/03/2024`.
pub(crate) static QUERY_DOCKET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        &[
            r"(?i)(?P<pourvoi>\b\d{2}-\d{2}\.\d{3,4}\b|\b\d{2}-\d{4,5}\b)",
            &format!(r"|(?P<rg>\b\d{{2}}{SLASH}\d{{5}}\b)"),
            r"|(?P<constit>\b\d{2,4}-\d{1,4})\s*(?:qpc|dc|lp|l|an|elec|ord|orga|fnr)\b",
            // Before the bare-digit branch, which would match `31399` alone.
            &format!(r"|(?P<cedh>\b\d{{3,5}}{SLASH}\d{{2}}\b)"),
            r"|(?P<lettres>\b\d{2,4}[a-z]{1,3}\d{3,6}\b)",
            &format!(r"|(?P<cjue>\b[ctf]-\d{{1,4}}{SLASH}\d{{2}}\b)"),
            r"|(?P<cnil>\b[a-z]{2,4}-\d{4}-\d{3}\b)",
            r"|(?P<ecli>\becli:[a-z]{2}:[a-z0-9]+:\d{4}:[a-z0-9.]+)",
            r"|(?P<nu>\b\d{5,10}\b)",
        ]
        .concat(),
    )
    .unwrap()
});

/// The branch a match came from and the courts its graphy can belong to, as
/// `jurisdiction_type` codes; empty when the graphy says nothing.
fn courts_of(caps: &regex::Captures) -> (&'static str, &'static [&'static str]) {
    for (group, courts) in [
        ("pourvoi", &["CC"][..]),
        ("rg", &["CA", "TJ", "TCOM"][..]),
        ("constit", &["CONSTIT"][..]),
        // `106/77` is Simmenthal as often as a Strasbourg application.
        ("cedh", &["CEDH", "CJUE"][..]),
        ("lettres", &[][..]),
        ("cjue", &["CJUE"][..]),
        ("cnil", &["CNIL"][..]),
        ("ecli", &[][..]),
        ("nu", &[][..]),
    ] {
        if caps.name(group).is_some() {
            return (group, courts);
        }
    }
    unreachable!("every branch of QUERY_DOCKET_RE is named")
}

/// L'ordinal nu d'un numéro du Tribunal des conflits. Le fonds l'écrit selon
/// l'époque (`00012` avant 2000, `3220` jusqu'en 2004, `C4174` ensuite) et les
/// praticiens indifféremment : ni le `C` ni les zéros de tête ne distinguent
/// deux décisions. Le suffixe, lui, en sépare deux (`2530B` n'est pas `02530`)
/// et reste.
pub fn tc_ordinal(num: &str) -> String {
    let plie = docket_key(num);
    let nu = plie
        .strip_prefix('c')
        .unwrap_or(&plie)
        .trim_start_matches('0');
    if nu.is_empty() {
        plie
    } else {
        nu.to_string()
    }
}

/// L'unité qui suit un nombre en fait une quantité : `500000 euros`, `35 %`.
static UNITE_SUIVANTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^[\s.,]*(?:",
        r"€|\$|%|eur\b|euros?\b|francs?\b|dollars?\b|usd\b|centimes?\b",
        r"|m²|m2\b|km\b|kg\b|tonnes?\b|hectares?\b",
        r"|habitants?\b|salari[ée]s?\b|emplois?\b",
        r"|heures?\b|jours?\b|semaines?\b|mois\b|ans?\b|ann[ée]es?\b",
        r")",
    ))
    .unwrap()
});

/// Ce qui, avant un nombre, en annonce le montant quand aucune unité ne suit :
/// `condamné à payer 500000`.
static MONTANT_PRECEDENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:",
        r"montants?|sommes?|indemnit[ée]s?|pr[ée]judices?|dommages?|int[ée]r[êe]ts?",
        r"|amendes?|astreintes?|cr[ée]ances?|dettes?|loyers?|chiffres?",
        r"|salaires?|r[ée]mun[ée]rations?|prix|valeurs?|co[ûu]ts?",
        r"|payer|verser|rembourser|allouer|[ée]valu[ée]e?s?|condamn[ée]e?s?",
        r")\b[^0-9]{0,24}$",
    ))
    .unwrap()
});

/// La marque de numérotation, qui rattrape un nombre précédé d'un mot d'argent.
///
/// Pas de `\b` de fin : `°` n'est pas un caractère de mot, donc aucune frontière
/// ne sépare `n°` de l'espace qui suit. `affaire` n'y est pas, « chiffre
/// d'affaires » le porterait.
static NUMERO_PRECEDENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:n°|n[°o]s?|num[ée]ros?|rg|pourvoi|requ[êe]te|dossier)[^0-9]{0,8}$")
        .unwrap()
});

/// What, just before a number, makes it the number of a text or of one of its
/// articles: `directive 2008/48`, `règlement 2016/679`, `article 1384`.
/// `ordonnance` is not there, a judge's order carries a docket number.
static TEXTE_PRECEDENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:articles?|art\.?|alin[ée]as?|directives?|r[èe]glements?",
        r"|d[ée]crets?|lois?|codes?)\s*(?:n°|no|n\.)?\s*(?:\(?(?:ue|ce|cee)\)?\s*)?$",
    ))
    .unwrap()
});

fn est_un_texte(query: &str, debut: usize) -> bool {
    TEXTE_PRECEDENT_RE.is_match(&query[..debut])
}

/// Le nombre trouvé en `[debut, fin[` est-il une quantité et non un rôle ?
///
/// Trois signaux par ordre de force : l'unité qui suit tranche seule, sinon la
/// marque de numérotation rattrape, sinon le mot d'argent écarte.
///
/// `500000` is also a Conseil d'État number: read as one, `indemnité de 500000
/// euros` would take the pure-reference weight and `truncate_fused` cut the rest.
fn est_une_quantite(query: &str, debut: usize, fin: usize) -> bool {
    if UNITE_SUIVANTE_RE.is_match(&query[fin..]) {
        return true;
    }
    let avant = &query[..debut];
    !NUMERO_PRECEDENT_RE.is_match(avant) && MONTANT_PRECEDENT_RE.is_match(avant)
}

/// Four digits standing alone: the shape of a year, and of the Conseil d'État
/// numbers before the eighties (`5911`, written `05911` by the fonds).
static QUATRE_CHIFFRES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{4}\b").unwrap());

/// A number the query carries: its canonical key, the courts its graphy can
/// belong to (empty: any), and whether it is weak, four digits beside a court
/// name, a shape a year shares, so it designates without ever declaring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryNumber {
    pub key: String,
    pub courts: &'static [&'static str],
    pub weak: bool,
}

/// The numbers a query carries, deduplicated, in order of appearance. Amounts
/// and the numbers of a text are left out, ECLIs go to [`query_eclis`].
pub fn query_numbers(query: &str) -> Vec<QueryNumber> {
    let mut out: Vec<QueryNumber> = Vec::new();
    let mut push = |key: String, courts: &'static [&'static str], weak: bool| {
        if !key.is_empty() && !out.iter().any(|n| n.key == key) {
            out.push(QueryNumber { key, courts, weak });
        }
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for caps in QUERY_DOCKET_RE.captures_iter(query) {
        let m = caps.get(0).unwrap();
        spans.push((m.start(), m.end()));
        let (group, courts) = courts_of(&caps);
        if group == "ecli" || est_un_texte(query, m.start()) {
            continue;
        }
        if m.as_str().bytes().all(|b| b.is_ascii_digit())
            && est_une_quantite(query, m.start(), m.end())
        {
            continue;
        }
        let number = match group {
            "constit" => &caps["constit"],
            _ => m.as_str(),
        };
        push(docket_key(number), courts, false);
    }
    let folded = crate::case_names::case_name_key(query);
    let words: Vec<&str> = folded.split(' ').collect();
    if !crate::citation::names_a_court(&words) {
        return out;
    }
    for m in QUATRE_CHIFFRES_RE.find_iter(query) {
        let year: u32 = m.as_str().parse().unwrap();
        if (1800..=2100).contains(&year)
            || spans.iter().any(|(s, e)| m.start() < *e && m.end() > *s)
            || est_une_quantite(query, m.start(), m.end())
            || est_un_texte(query, m.start())
        {
            continue;
        }
        push(m.as_str().to_string(), &[], true);
        push(format!("0{}", m.as_str()), &[], true);
    }
    out
}

/// The ECLIs a query carries, upper-cased as the fonds stores them.
pub fn query_eclis(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for caps in QUERY_DOCKET_RE.captures_iter(query) {
        if let Some(m) = caps.name("ecli") {
            let ecli = m.as_str().to_uppercase();
            if !out.contains(&ecli) {
                out.push(ecli);
            }
        }
    }
    out
}

/// Does the query designate its number AS a number? The shape (`19-20.682`,
/// `C-369/90`) can be nothing else; quotes and a numbering mark state the intent
/// on a bare run, which otherwise shares its form with an amount, a postcode or
/// a year. Digits stuck to a name and a subject (`401234 Dupont délai de
/// recours`) are a clue and not the request.
pub fn declares_reference_explicitly(query: &str) -> bool {
    QUERY_DOCKET_RE
        .find_iter(query)
        .any(|m| marque_explicite(query, &m) && !est_un_texte(query, m.start()))
}

/// The whole query is a run of six digits or more, a length no postcode and no
/// year reach: such a run nearly always names a decision of the fonds, where a
/// five-digit one stays bare.
pub fn bare_number_declares(query: &str) -> bool {
    let seule = query.trim();
    seule.len() >= 6 && seule.bytes().all(|b| b.is_ascii_digit()) && is_bare_number(seule)
}

/// The number is the whole query. Too weak to pick a search mode on its own:
/// `69003` and `13008` are postcodes.
pub fn is_bare_number(query: &str) -> bool {
    let seule = query.trim();
    QUERY_DOCKET_RE
        .find_iter(query)
        .any(|m| m.as_str() == seule)
}

fn marque_explicite(query: &str, m: &regex::Match) -> bool {
    m.as_str().bytes().any(|b| !b.is_ascii_digit())
        || (query[..m.start()].ends_with('"') && query[m.end()..].starts_with('"'))
        || NUMERO_PRECEDENT_RE.is_match(&query[..m.start()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(query: &str) -> Vec<String> {
        query_numbers(query).into_iter().map(|n| n.key).collect()
    }

    #[test]
    fn les_trois_epoques_du_tribunal_des_conflits_donnent_le_meme_ordinal() {
        for n in ["00012", "12", "C12", "c 12", "0012"] {
            assert_eq!(tc_ordinal(n), "12", "numéro {n:?}");
        }
        assert_eq!(tc_ordinal("C4174"), "4174");
        assert_eq!(tc_ordinal("3220"), "3220");
    }

    #[test]
    fn le_suffixe_separe_deux_decisions_du_tribunal_des_conflits() {
        assert_ne!(tc_ordinal("2530B"), tc_ordinal("02530"));
        assert_eq!(tc_ordinal("2530B"), "2530b");
    }

    #[test]
    fn les_graphies_dun_numero_tombent_sur_la_meme_cle() {
        for n in ["19-20.312", "19-20312", "1920312", "19 20 312", "19/20.312"] {
            assert_eq!(docket_key(n), "1920312", "graphie {n:?}");
        }
        for n in ["22/01772", "22-01772", "2201772", "22 01772"] {
            assert_eq!(docket_key(n), "2201772", "graphie {n:?}");
        }
    }

    #[test]
    fn la_cle_garde_les_lettres_et_est_un_point_fixe() {
        assert_eq!(docket_key("C-806/18"), "c80618");
        assert_eq!(docket_key("c80618"), "c80618");
        assert_eq!(docket_key(&docket_key("19-20.312")), "1920312");
        assert_eq!(docket_key("---"), "");
    }

    #[test]
    fn extraction_depuis_la_requete() {
        assert_eq!(keys("pourvoi 12-34.567"), ["1234567"]);
        assert_eq!(keys("Douai 21/01234 sociale"), ["2101234"]);
        assert_eq!(keys("CE 412345"), ["412345"]);
        assert!(keys("congés payés").is_empty());
    }

    /// The role number of the commercial and economic-activity courts runs to
    /// ten digits.
    #[test]
    fn le_role_a_dix_chiffres_des_tribunaux_de_commerce() {
        assert_eq!(keys("2021012345"), ["2021012345"]);
        assert_eq!(keys("TAE Paris RG n° 2021012345"), ["2021012345"]);
        assert_eq!(
            keys("tribunal de commerce de Lille 2025054321"),
            ["2025054321"]
        );
    }

    /// The number is taken whatever surrounds it, prose included, and a money
    /// word after it does not drop it.
    #[test]
    fn a_number_is_read_whatever_surrounds_it() {
        assert_eq!(
            keys("2301234 Melun titre de séjour salarié autorisation de travail"),
            ["2301234"]
        );
        assert_eq!(
            keys("\"412345\" cession à un prix significativement inférieur à la valeur vénale"),
            ["412345"]
        );
        assert_eq!(
            keys("\"401234\" prescription quadriennale créance indemnitaire"),
            ["401234"]
        );
        assert_eq!(keys("requête n° 12345/08 Dupont c. France"), ["1234508"]);
        assert_eq!(keys("CEDH 54321/21"), ["5432121"]);
        assert!(keys("décision du 15/06/2021 sur le permis").is_empty());
    }

    /// An amount stays out of the keys, even when a Conseil d'État number
    /// carries the same digits.
    #[test]
    fn un_montant_ne_prend_pas_le_role_qui_porte_le_meme_nombre() {
        for q in [
            "indemnité de 500000 euros",
            "préjudice de 200000 euros",
            "astreinte de 120000 euros",
            "condamné à payer 500000",
            "chiffre d'affaires de 850000 €",
            "commune de 120000 habitants",
        ] {
            assert!(keys(q).is_empty(), "requête {q:?}");
        }
    }

    /// La marque de numérotation rattrape ce que le mot d'argent écarterait, et
    /// un rôle structuré n'est jamais concerné.
    #[test]
    fn le_garde_fou_epargne_les_vrais_roles() {
        assert_eq!(keys("dommages n° 250000"), ["250000"]);
        assert_eq!(
            keys("indemnité de 150000 euros pourvoi 12-34.567"),
            ["1234567"]
        );
        assert_eq!(
            keys("\"412345\" Société Alpha cession à un prix inférieur à la valeur vénale"),
            ["412345"]
        );
    }

    /// Five digits enter, a postcode too, which declares nothing and so adds a
    /// key rather than replacing the search; four stay out, where a year falls.
    #[test]
    fn les_bornes_des_chiffres_nus() {
        assert_eq!(keys("12345"), ["12345"]);
        assert_eq!(
            keys("bail commercial 69003 Lyon"),
            ["69003"],
            "le code postal entre en clé"
        );
        assert!(
            !declares_reference_explicitly("bail commercial 69003 Lyon")
                && !is_bare_number("bail commercial 69003 Lyon"),
            "mais il ne fait pas loi"
        );
        assert!(keys("arrêt de 1970").is_empty());
        assert!(keys("12345678901").is_empty());
    }

    #[test]
    fn extraction_des_graphies_a_lettres() {
        assert_eq!(keys("CAA Paris 20PA01234"), ["20pa01234"]);
        assert_eq!(keys("caa douai n° 21da00123"), ["21da00123"]);
        assert_eq!(keys("98PA1234"), ["98pa1234"]);
        assert_eq!(keys("TCOM Paris 2010J00123"), ["2010j00123"]);
        assert_eq!(keys("CJUE C-123/95 Schmidt"), ["c12395"]);
        assert_eq!(keys("T-12/99"), ["t1299"]);
        assert_eq!(keys("délibération SAN-2021-012"), ["san2021012"]);
    }

    /// Ce qu'une requête juridique écrit couramment et qui n'est pas un numéro de
    /// décision : la clé exacte rendrait une jambe vide, et sur une référence pure
    /// un verdict d'absence qui serait faux.
    #[test]
    fn ni_un_article_ni_un_texte_ne_sont_un_numero() {
        for q in [
            "article L761-1 du code de justice administrative",
            "L. 531-1",
            "décret 2020-1310",
            "loi 2016-1547 de modernisation de la justice",
            "2024-02-13",
            "arrêt du 13 février 2024",
        ] {
            assert!(keys(q).is_empty(), "requête {q:?}");
        }
    }

    /// The number of a text or of an article designates no decision, whatever
    /// its graphy: `2008/48` has the Strasbourg shape.
    #[test]
    fn le_numero_d_un_texte_n_est_pas_celui_d_une_decision() {
        for q in [
            "information de l'emprunteur article 10 directive 2008/48",
            "règlement (UE) 2016/679 données personnelles",
            "directive n° 2008/48",
            "conseil d'état article 1242",
        ] {
            assert!(keys(q).is_empty(), "requête {q:?}");
            assert!(!declares_reference_explicitly(q), "requête {q:?}");
        }
        assert_eq!(keys("ordonnance n° 2401234"), ["2401234"]);
    }

    #[test]
    fn deux_graphies_du_meme_numero_dans_une_requete_donnent_une_cle() {
        assert_eq!(keys("19-20312 ou 1920312"), ["1920312"]);
    }

    /// The four ways to designate a number as a number, against the bare digits
    /// thrown into prose.
    #[test]
    fn la_requete_dit_ou_ne_dit_pas_que_le_nombre_est_un_numero() {
        for q in [
            "12-34.567",
            "Douai 21/01234 chambre sociale",
            "CJUE, 1er mars 1995, C-123/95",
            "\"123456\"",
            "\"123456\" OU \"234567\"",
            "TA Melun, 2 mai 2024, n° 2401234",
        ] {
            assert!(declares_reference_explicitly(q), "numéro déclaré {q:?}");
        }
        assert!(is_bare_number("1701234"));
        assert!(is_bare_number(" 69003 "));
        assert!(!declares_reference_explicitly("1701234"));
        for q in [
            "412345 Société Alpha",
            "décision 401234 Dupont délai de recours",
            "barème 300000 accident du travail",
            "licenciement sans cause réelle",
        ] {
            assert!(
                !declares_reference_explicitly(q) && !is_bare_number(q),
                "chiffre nu ou prose {q:?}"
            );
        }
    }

    /// Six digits standing alone are a docket number and nothing else ; five are
    /// still a postcode, and a number inside a question is a clue, not the request.
    #[test]
    fn le_chiffre_nu_declare_a_partir_de_six() {
        for q in ["2501234", " 1701234 ", "345678", "2021012345"] {
            assert!(bare_number_declares(q), "numéro nu {q:?}");
        }
        for q in [
            "69003",
            "13008",
            "5432",
            "401234 Dupont délai de recours",
            "licenciement sans cause réelle",
        ] {
            assert!(!bare_number_declares(q), "ni numéro nu ni assez long {q:?}");
        }
    }

    /// Four digits beside a court name are a number the fonds may write with a
    /// leading zero, weak because a year has the same shape: they buy a card,
    /// never a mode. Alone, or beside a year, they stay a year.
    #[test]
    fn quatre_chiffres_pres_d_une_cour_sont_un_numero_faible() {
        let faibles = query_numbers("5432 conseil d'etat 1975");
        assert_eq!(
            faibles.iter().map(|n| n.key.as_str()).collect::<Vec<_>>(),
            ["5432", "05432"]
        );
        assert!(faibles.iter().all(|n| n.weak));
        assert!(!is_bare_number("5432"));
        assert!(!declares_reference_explicitly("5432 conseil d'etat 1975"));
        for q in [
            "5432",
            "conseil d'état 1920",
            "conseil d'état 2023 permis de construire",
            "CE 3000 euros",
        ] {
            assert!(keys(q).is_empty(), "requête {q:?}");
        }
        assert!(query_numbers("CE 412345 1975").iter().all(|n| !n.weak));
    }

    /// The graphy names the order: a pourvoi is the Cour de cassation, an RG a
    /// judicial court, `2012-345 QPC` the Conseil constitutionnel, whose key the
    /// number of another court may share.
    #[test]
    fn la_graphie_dit_a_quelles_cours_le_numero_appartient() {
        let un = |q: &str| {
            let mut n = query_numbers(q);
            assert_eq!(n.len(), 1, "requête {q:?}");
            n.pop().unwrap()
        };
        assert_eq!(un("12-34.567").courts, ["CC"]);
        assert!(un("1234567").courts.is_empty());
        assert_eq!(un("21/01234").courts, ["CA", "TJ", "TCOM"]);
        assert_eq!(un("12345/98").courts, ["CEDH", "CJUE"]);
        assert_eq!(un("C-123/95").courts, ["CJUE"]);
        assert!(un("20PA01234").courts.is_empty());
        let qpc = un("2012-345 QPC 1er juin 2012");
        assert_eq!(
            (qpc.key.as_str(), qpc.courts),
            ("2012345", &["CONSTIT"][..])
        );
        assert_eq!(
            un("85-123 dc conseil constitutionnel 1er juin 1985").key,
            "85123"
        );
        assert_eq!(un("\"2025-123 DC\"").key, "2025123");
        assert!(is_bare_number("2012-345 QPC"));
        assert!(declares_reference_explicitly("2012-345 QPC"));
    }

    #[test]
    fn l_ecli_est_une_reference_a_part() {
        assert_eq!(
            query_eclis("ecli:fr:ccass:2012:c100123"),
            ["ECLI:FR:CCASS:2012:C100123"]
        );
        assert_eq!(
            query_eclis("ECLI:EU:T:2020:123 rejet"),
            ["ECLI:EU:T:2020:123"]
        );
        assert!(keys("ecli:fr:ccass:2012:c100123").is_empty());
        assert!(is_bare_number("ECLI:FR:CCASS:2012:C100123"));
        assert!(declares_reference_explicitly("ecli:fr:ccass:2012:c100123"));
        assert!(query_eclis("congés payés").is_empty());
    }

    #[test]
    fn spaces_around_a_slash_leave_the_number_as_it_was() {
        let raw = include_str!("../tests/fixtures/oracle/docket_keys.json");
        let cases: Vec<KeyCase> = serde_json::from_str(raw).expect("fixture docket_keys");
        let mut recognised = 0;
        for num in cases
            .iter()
            .map(|c| c.num.as_str())
            .filter(|n| n.contains('/'))
        {
            for mark in ["", "n° ", "RG "] {
                let glued = format!("{mark}{num}");
                for spaced in [" / ", "/ ", " /"] {
                    let q = format!("{mark}{}", num.replace('/', spaced));
                    assert_eq!(keys(&q), keys(&glued), "{q:?}");
                    assert_eq!(is_bare_number(&q), is_bare_number(&glued), "{q:?}");
                    assert_eq!(
                        declares_reference_explicitly(&q),
                        declares_reference_explicitly(&glued),
                        "{q:?}"
                    );
                }
            }
            recognised += usize::from(!keys(num).is_empty());
        }
        assert!(
            recognised >= 40,
            "{recognised} slashed numbers of the oracle recognised"
        );
        assert_eq!(keys("RG 21 / 01234"), ["2101234"]);
        assert!(declares_reference_explicitly("RG 21 / 01234"));
    }

    #[derive(serde::Deserialize)]
    struct KeyCase {
        num: String,
        key: String,
    }

    #[test]
    fn parite_avec_la_fonction_sql_lj_docket_keys() {
        let raw = include_str!("../tests/fixtures/oracle/docket_keys.json");
        let cases: Vec<KeyCase> = serde_json::from_str(raw).expect("fixture docket_keys");
        for c in &cases {
            assert_eq!(docket_key(&c.num), c.key, "num {:?}", c.num);
        }
    }
}
