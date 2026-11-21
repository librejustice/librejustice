//! A decision page of Legalis into a [`Decision`] and its `source_fields`: the court, the room
//! and the date from the page's title, the text from its body, less the lines Legalis adds.

use serde_json::{json, Value};

use lj_core::decision::Decision;

/// What a decision page shows.
#[derive(Debug, Clone, PartialEq)]
pub struct LegalisPage {
    /// The WordPress post, the `?p=` of the page's short link.
    pub post_id: u64,
    /// `Cour d'appel de Paris, Pôle 5 - Ch. 11, arrêt du 10 avril 2026`.
    pub title: String,
    /// The body, one paragraph a line, its keywords left out.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalisRefused {
    NoDate,
    NoText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LegalisDecision {
    pub decision: Decision,
    pub source_fields: Value,
}

/// The court a title opens on, as the extraction reads a court.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalisCourt {
    pub jurisdiction_type: &'static str,
    /// A judicial court's location, `ca_paris`, `tgi_nanterre`.
    pub location: Option<String>,
    pub name: String,
    /// The bench a tribunal correctionnel or pour enfants sits as inside its court.
    pub bench: Option<&'static str>,
}

/// The non-blank characters a decision holds at least.
const DECISION_FLOOR: usize = 200;

pub fn legalis_source_uid(post_id: u64) -> String {
    format!("legalis/{post_id}")
}

/// The words a title opens on. No location prefix: the type alone names the court, or its name
/// does.
struct Family {
    words: &'static [&'static str],
    jurisdiction_type: &'static str,
    location: Option<&'static str>,
    name: &'static str,
    bench: Option<&'static str>,
}

/// A bench of the tribunal de grande instance until 2020, of the tribunal judiciaire since.
const DATED: &str = "dated";
const CORRECTIONAL: &str = "chambre correctionnelle";
/// Cities the Défenseur des droits misspells, with the spelling the referential holds.
const MISSPELT: [(&str, &str, &str); 3] = [
    ("bethunes", "bethune", "de Béthune"),
    ("chatelleraut", "chatellerault", "de Châtellerault"),
    ("lons_le_saulnier", "lons_le_saunier", "de Lons-le-Saunier"),
];
const TASS: &str = "tass";

/// A tribunal des affaires de sécurité sociale named after its département sat at its préfecture.
const TASS_SEATS: [(&str, &str, &str); 31] = [
    ("allier", "moulins", "de Moulins"),
    (
        "alpes_de_haute_provence",
        "digne_les_bains",
        "de Digne-les-Bains",
    ),
    ("aube", "troyes", "de Troyes"),
    ("bas_rhin", "strasbourg", "de Strasbourg"),
    ("bouches_du_rhone", "marseille", "de Marseille"),
    ("cote_d_or", "dijon", "de Dijon"),
    ("cotes_d_armor", "saint_brieuc", "de Saint-Brieuc"),
    ("essonne", "evry", "d'Évry"),
    ("gard", "nimes", "de Nîmes"),
    ("gironde", "bordeaux", "de Bordeaux"),
    ("haute_savoie", "annecy", "d'Annecy"),
    ("hautes_alpes", "gap", "de Gap"),
    ("hautes_pyrenees", "tarbes", "de Tarbes"),
    ("hauts_de_seine", "nanterre", "de Nanterre"),
    ("herault", "montpellier", "de Montpellier"),
    ("ille_et_vilaine", "rennes", "de Rennes"),
    ("loir_et_cher", "blois", "de Blois"),
    ("loiret", "orleans", "d'Orléans"),
    ("lozere", "mende", "de Mende"),
    ("meurthe_et_moselle", "nancy", "de Nancy"),
    ("moselle", "metz", "de Metz"),
    ("nord", "lille", "de Lille"),
    ("puy_de_dome", "clermont_ferrand", "de Clermont-Ferrand"),
    ("sarthe", "le_mans", "du Mans"),
    ("savoie", "chambery", "de Chambéry"),
    ("seine_saint_denis", "bobigny", "de Bobigny"),
    ("tarn_et_garonne", "montauban", "de Montauban"),
    ("val_d_oise", "pontoise", "de Pontoise"),
    ("var", "toulon", "de Toulon"),
    ("yonne", "auxerre", "d'Auxerre"),
    ("yvelines", "versailles", "de Versailles"),
];

const fn named_family(
    words: &'static [&'static str],
    jurisdiction_type: &'static str,
    location: Option<&'static str>,
    name: &'static str,
) -> Family {
    Family {
        words,
        jurisdiction_type,
        location,
        name,
        bench: None,
    }
}

const FAMILIES: &[Family] = &[
    named_family(&["cour", "d", "appel"], "CA", Some("ca"), "Cour d'appel"),
    named_family(
        &["cour", "administrative", "d", "appel"],
        "CAA",
        None,
        "Cour administrative d'appel",
    ),
    named_family(
        &["cour", "de", "cassation"],
        "CC",
        None,
        "Cour de cassation",
    ),
    named_family(
        &["le", "conseil", "d", "etat"],
        "CE",
        None,
        "Conseil d'État",
    ),
    named_family(&["conseil", "d", "etat"], "CE", None, "Conseil d'État"),
    named_family(
        &["le", "conseil", "constitutionnel"],
        "CONSTIT",
        None,
        "Conseil constitutionnel",
    ),
    named_family(
        &["conseil", "constitutionnel"],
        "CONSTIT",
        None,
        "Conseil constitutionnel",
    ),
    named_family(
        &["tribunal", "des", "conflits"],
        "TC",
        None,
        "Tribunal des conflits",
    ),
    named_family(
        &["cour", "de", "justice", "de", "l", "union", "europeenne"],
        "CJUE",
        None,
        "Cour de justice de l'Union européenne",
    ),
    named_family(
        &["cour", "de", "justice", "des", "communautes", "europeennes"],
        "CJUE",
        None,
        "Cour de justice de l'Union européenne",
    ),
    named_family(
        &["cjue"],
        "CJUE",
        None,
        "Cour de justice de l'Union européenne",
    ),
    named_family(
        &["cour", "europeenne", "des", "droits", "de", "l", "homme"],
        "CEDH",
        None,
        "Cour européenne des droits de l'homme",
    ),
    named_family(
        &["tribunal", "administratif"],
        "TA",
        None,
        "Tribunal administratif",
    ),
    named_family(
        &["tribunal", "administrative"],
        "TA",
        None,
        "Tribunal administratif",
    ),
    named_family(
        &["tribunal", "de", "grande", "instance"],
        "TJ",
        Some("tgi"),
        "Tribunal de grande instance",
    ),
    named_family(&["tgi"], "TJ", Some("tgi"), "Tribunal de grande instance"),
    named_family(
        &["tribunal", "judiciaire"],
        "TJ",
        Some("tj"),
        "Tribunal judiciaire",
    ),
    named_family(
        &["tribunal", "judicaire"],
        "TJ",
        Some("tj"),
        "Tribunal judiciaire",
    ),
    named_family(
        &["tribunal", "de", "premiere", "instance"],
        "TJ",
        Some("tpi"),
        "Tribunal de première instance",
    ),
    named_family(
        &["tribunal", "d", "instance"],
        "TJ",
        Some("ti"),
        "Tribunal d'instance",
    ),
    named_family(&["ti"], "TJ", Some("ti"), "Tribunal d'instance"),
    named_family(
        &["juridiction", "de", "proximite"],
        "TJ",
        Some("prox"),
        "Juridiction de proximité",
    ),
    Family {
        bench: Some(CORRECTIONAL),
        ..named_family(
            &["tribunal", "correctionnel"],
            "TJ",
            Some(DATED),
            "Tribunal correctionnel",
        )
    },
    Family {
        bench: Some("juge des enfants"),
        ..named_family(
            &["tribunal", "pour", "enfants"],
            "TJ",
            Some(DATED),
            "Tribunal pour enfants",
        )
    },
    named_family(
        &["tribunal", "des", "affaires", "de", "securite", "sociale"],
        "TJ",
        Some(TASS),
        "Tribunal des affaires de sécurité sociale",
    ),
    named_family(
        &["tribunal", "des", "affaires", "de", "securite", "sociales"],
        "TJ",
        Some(TASS),
        "Tribunal des affaires de sécurité sociale",
    ),
    named_family(
        &["tribunal", "de", "commerce"],
        "TCOM",
        Some("tcom"),
        "Tribunal de commerce",
    ),
    named_family(
        &["conseil", "de", "prud", "hommes"],
        "CPH",
        Some("cph"),
        "Conseil de prud'hommes",
    ),
    named_family(
        &["conseil", "des", "prud", "hommes"],
        "CPH",
        Some("cph"),
        "Conseil de prud'hommes",
    ),
];

/// The words that open a second court after a city.
const COURT_HEADS: [&str; 2] = ["tribunal", "juridiction"];

/// The cities whose tribunaux d'instance and juridictions de proximité sit by arrondissement.
const ARRONDISSEMENTS: [&str; 3] = ["paris", "lyon", "marseille"];

/// The words that end a city in a title that does not close its court with a comma.
const AFTER_CITY: &[&str] = &[
    "tribunal",
    "juridiction",
    "police",
    "affaires",
    "requete",
    "deuxieme",
    "troisieme",
    "quatrieme",
    "cinquieme",
    "sixieme",
    "septieme",
    "huitieme",
    "neuvieme",
    "dixieme",
    "pole",
    "ch",
    "chambre",
    "chambres",
    "section",
    "sec",
    "arret",
    "arrets",
    "jugement",
    "jugements",
    "ordonnance",
    "ordonnances",
    "decision",
    "audience",
    "formation",
    "refere",
    "deliberation",
    "sentence",
    "avis",
    "service",
    "juge",
    "president",
    "premiere",
    "greffe",
    "statuant",
];

const MONTHS: [&str; 12] = [
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

/// A word of a title, folded, and where it sits in characters.
struct Word {
    folded: String,
    start: usize,
    end: usize,
}

fn words(chars: &[char]) -> Vec<Word> {
    let alnum = |c: char| lj_fold::fold_one(c).is_ascii_alphanumeric();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if !alnum(chars[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < chars.len() && alnum(chars[at]) {
            at += 1;
        }
        out.push(Word {
            folded: chars[start..at]
                .iter()
                .map(|&c| lj_fold::fold_one(c).to_ascii_lowercase())
                .collect(),
            start,
            end: at,
        });
    }
    out
}

/// The first `day month year` of a title, as an ISO date.
pub fn legalis_date(title: &str) -> Option<String> {
    let chars: Vec<char> = title.chars().collect();
    words(&chars).windows(3).find_map(|w| {
        let day: i8 = w[0]
            .folded
            .strip_suffix("er")
            .unwrap_or(&w[0].folded)
            .parse()
            .ok()?;
        let month = i8::try_from(MONTHS.iter().position(|m| *m == w[1].folded)? + 1).ok()?;
        let year: i16 = w[2].folded.parse().ok()?;
        let date = jiff::civil::Date::new(year, month, day).ok()?;
        (year >= 1900).then(|| date.to_string())
    })
}

fn family_at(words: &[Word]) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| {
        words.len() >= f.words.len() && f.words.iter().zip(words).all(|(a, b)| *a == b.folded)
    })
}

/// The number an ordinal carries: `15eme`, `1er`, `2e`, `xxeme`.
fn ordinal(word: &str) -> Option<u32> {
    let arabic = ["eme", "er", "e"]
        .iter()
        .find_map(|suffix| word.strip_suffix(suffix)?.parse().ok());
    arabic.or_else(|| {
        let roman = word.strip_suffix("eme")?;
        let value = |c| match c {
            'i' => Some(1),
            'v' => Some(5),
            'x' => Some(10),
            _ => None,
        };
        let digits: Vec<u32> = roman.chars().map(value).collect::<Option<_>>()?;
        let next = digits.iter().skip(1).map(Some).chain([None]);
        let total = digits.iter().zip(next).fold(0i64, |sum, (&d, n)| match n {
            Some(&n) if n > d => sum - i64::from(d),
            _ => sum + i64::from(d),
        });
        u32::try_from(total).ok().filter(|n| *n > 0)
    })
}

fn ends_city(word: &str) -> bool {
    AFTER_CITY.contains(&word)
        || word.starts_with(|c: char| c.is_ascii_digit())
        || ordinal(word).is_some()
}

fn arrondissement_word(word: &Word) -> bool {
    matches!(word.folded.as_str(), "arr" | "arrondissement")
}

/// Where a court sits, read from the words after its family's.
struct Place {
    /// `la_roche_sur_yon`, `le_mans` for « du Mans », `paris_2e_arrondissement`.
    slug: String,
    /// As the name of the court continues: `du Mans`, `de Paris 2e arrondissement`.
    named: String,
    /// The first word past the place.
    end: usize,
}

/// `arrondissements` reads « Paris 15ème arr. » and « 2ème arrondissement de Paris » as the
/// arrondissement's court.
fn place(chars: &[char], rest: &[Word], arrondissements: bool) -> Option<Place> {
    let leading_at = usize::from(rest.first().is_some_and(|w| w.folded == "du"));
    let leading = arrondissements
        .then(|| rest.get(leading_at).and_then(|w| ordinal(&w.folded)))
        .flatten();
    let mut at = 0;
    if leading.is_some() {
        at = leading_at + 1;
        at += usize::from(rest.get(at).is_some_and(arrondissement_word));
    }
    let preposition = rest
        .get(at)
        .filter(|w| matches!(w.folded.as_str(), "de" | "d" | "du" | "des"));
    if leading.is_some() && preposition.is_none() {
        return None;
    }
    let from = at + usize::from(preposition.is_some());
    let city: Vec<&Word> = rest[from..]
        .iter()
        .take_while(|w| !ends_city(&w.folded))
        .collect();
    let (first, last) = (city.first()?, city.last()?);
    let mut end = from + city.len();
    let article = match preposition.map(|p| p.folded.as_str()) {
        Some("du") => "le_",
        Some("des") => "les_",
        _ => "",
    };
    let words: Vec<&str> = city
        .iter()
        .map(|w| match w.folded.as_str() {
            "st" => "saint",
            "ste" => "sainte",
            word => word,
        })
        .collect();
    let mut slug = format!("{article}{}", words.join("_"));
    let city_text = || -> String {
        chars[first.start..last.end]
            .iter()
            .collect::<String>()
            .replace('\u{2019}', "'")
    };
    let trailing = (arrondissements && leading.is_none())
        .then(|| rest.get(end).and_then(|w| ordinal(&w.folded)))
        .flatten()
        .filter(|_| ARRONDISSEMENTS.contains(&slug.as_str()));
    if trailing.is_some() {
        end += 1;
        end += usize::from(rest.get(end).is_some_and(arrondissement_word));
    }
    let named = match leading.or(trailing) {
        Some(_) if !ARRONDISSEMENTS.contains(&slug.as_str()) => return None,
        Some(n) => {
            let suffix = if n == 1 { "er" } else { "e" };
            slug = format!("{slug}_{n}{suffix}_arrondissement");
            format!("de {} {n}{suffix} arrondissement", city_text())
        }
        None => match preposition {
            Some(p) => chars[p.start..last.end]
                .iter()
                .collect::<String>()
                .replace('\u{2019}', "'"),
            None => format!("de {}", city_text()),
        },
    };
    Some(Place { slug, named, end })
}

/// The court a title opens on, `None` for one no family names or a second court leaves in
/// doubt. `date` decides between the tribunal de grande instance and the tribunal judiciaire.
pub fn legalis_court(title: &str, date: &str) -> Option<LegalisCourt> {
    let head = title.split([',', '(']).next().unwrap_or(title);
    let chars: Vec<char> = head.chars().collect();
    let words = words(&chars);
    let family = family_at(&words)?;
    if matches!(
        family.jurisdiction_type,
        "CC" | "CE" | "CONSTIT" | "TC" | "CJUE" | "CEDH"
    ) {
        return Some(LegalisCourt {
            jurisdiction_type: family.jurisdiction_type,
            location: None,
            name: family.name.to_string(),
            bench: None,
        });
    }
    let rest = &words[family.words.len()..];
    let mut place = place(&chars, rest, matches!(family.location, Some("ti" | "prox")))?;
    if let Some((_, slug, named)) = MISSPELT.iter().find(|(s, _, _)| *s == place.slug) {
        place.slug = (*slug).into();
        place.named = (*named).into();
    }
    match family.location {
        // The only conseil de prud'hommes at a Saint-Denis sits on La Réunion.
        Some("cph") if place.slug == "saint_denis" => {
            place.slug = "saint_denis_de_la_reunion".into();
            place.named = "de Saint-Denis de la Réunion".into();
        }
        Some(TASS) => {
            let departement = ["le_", "la_", "les_", "l_"]
                .iter()
                .find_map(|article| place.slug.strip_prefix(article))
                .unwrap_or(&place.slug);
            let seat = TASS_SEATS
                .iter()
                .find(|(d, _, _)| *d == departement)
                .copied();
            if let Some((_, slug, named)) = seat {
                place.slug = slug.into();
                place.named = named.into();
            }
        }
        _ => {}
    }
    let second = rest
        .get(place.end)
        .filter(|w| COURT_HEADS.contains(&w.folded.as_str()));
    let (family, bench) = match second {
        None => (family, family.bench),
        Some(_) => {
            let second = family_at(&rest[place.end..])?;
            match (family.location, second.location) {
                (Some("tgi" | "tj"), Some(DATED)) if second.bench == Some(CORRECTIONAL) => {
                    (family, second.bench)
                }
                (Some("ti"), Some("prox")) => (second, None),
                _ => return None,
            }
        }
    };
    let (prefix, name) = match family.location {
        Some(DATED) if date < "2020-01-01" => (Some("tgi"), "Tribunal de grande instance"),
        Some(DATED) => (Some("tj"), "Tribunal judiciaire"),
        location => (location, family.name),
    };
    Some(LegalisCourt {
        jurisdiction_type: family.jurisdiction_type,
        location: prefix.map(|prefix| format!("{prefix}_{}", place.slug)),
        name: format!("{name} {}", place.named),
        bench,
    })
}

/// The room a title names between its court and the kind of decision, `Pôle 5 - Ch. 11`.
fn room(title: &str) -> Option<String> {
    let parts: Vec<&str> = title.split(',').map(str::trim).collect();
    let room = parts.get(1..parts.len().saturating_sub(1))?.join(", ");
    (!room.is_empty()).then_some(room)
}

/// A line Legalis writes under a decision: `Some(origin)` for the source it took the text from,
/// `Some(None)` for the link to its own presentation.
fn trailer(line: &str) -> Option<Option<String>> {
    let folded = lj_fold::slug(line, ' ');
    if folded.contains("notre presentation") || folded.contains("notre presenation") {
        return Some(None);
    }
    let (head, origin) = line.split_once(':')?;
    (lj_fold::slug(head, ' ') == "source").then(|| Some(origin.trim().to_string()))
}

fn decision_of(
    post_id: u64,
    court: Option<&LegalisCourt>,
    date: &str,
    room: Option<String>,
    text: String,
) -> Decision {
    Decision {
        source_uid: legalis_source_uid(post_id),
        member_name: post_id.to_string(),
        ecli: None,
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: court.map(|c| c.name.clone()),
        jurisdiction_type: court.map(|c| c.jurisdiction_type.to_string()),
        jurisdiction_location: court.and_then(|c| c.location.clone()),
        numero_dossier: None,
        numero_dossiers: None,
        numero_role: None,
        date_lecture: Some(date.to_string()),
        date_audience: None,
        date_mise_jour: None,
        formation: match (court.and_then(|c| c.bench), room) {
            (Some(bench), Some(room)) => Some(format!("{bench}, {room}")),
            (bench, room) => room.or(bench.map(str::to_string)),
        },
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

/// The decision a page carries; a court no family names leaves it without a type.
pub fn legalis_decision(page: &LegalisPage) -> Result<LegalisDecision, LegalisRefused> {
    let date = legalis_date(&page.title).ok_or(LegalisRefused::NoDate)?;
    let mut lines: Vec<&str> = page.text.lines().collect();
    let mut origin = None;
    while let Some(found) = lines.last().and_then(|l| trailer(l)) {
        origin = origin.or(found);
        lines.pop();
    }
    let text = lines.join("\n");
    if text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(LegalisRefused::NoText);
    }
    let court = legalis_court(&page.title, &date);
    let source_fields = json!({
        "post_id": page.post_id,
        "title": page.title,
        "origin": origin,
    });
    Ok(LegalisDecision {
        decision: decision_of(page.post_id, court.as_ref(), &date, room(&page.title), text),
        source_fields,
    })
}

/// Branch of [`Decision::from_source_fields`] for `legalis/`, on the title the parse stored.
pub(crate) fn from_source_fields_legalis(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let post_id = source_fields
        .get("post_id")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("source_fields legalis without a post ({source_uid})"));
    let title = source_fields
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("source_fields legalis without a title ({source_uid})"));
    let date = legalis_date(title)
        .unwrap_or_else(|| panic!("source_fields legalis without a date ({source_uid})"));
    decision_of(
        post_id,
        legalis_court(title, &date).as_ref(),
        &date,
        room(title),
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn court(title: &str) -> Option<(String, Option<String>, String)> {
        let date = legalis_date(title).unwrap_or_default();
        legalis_court(title, &date).map(|c| (c.jurisdiction_type.to_string(), c.location, c.name))
    }

    #[test]
    fn a_title_gives_its_court_as_the_extraction_reads_it() {
        assert_eq!(
            court("Cour d\u{2019}appel d\u{2019}Aix-en-Provence, 1ère ch., arrêt du 3 mars 2015"),
            Some((
                "CA".into(),
                Some("ca_aix_en_provence".into()),
                "Cour d'appel d'Aix-en-Provence".into()
            ))
        );
        assert_eq!(
            court("Tribunal de Grande Instance de Paris 3ème chambre, 2ème section, Jugement du 25 avril 2003"),
            Some((
                "TJ".into(),
                Some("tgi_paris".into()),
                "Tribunal de grande instance de Paris".into()
            ))
        );
        assert_eq!(
            court("Tribunal de grande instance de Lille Ordonnance de référé du 11 juillet 2000"),
            Some((
                "TJ".into(),
                Some("tgi_lille".into()),
                "Tribunal de grande instance de Lille".into()
            ))
        );
        assert_eq!(
            court("Tribunal de commerce de Nanterre, 1ère ch., jugement du 7 juin 2023"),
            Some((
                "TCOM".into(),
                Some("tcom_nanterre".into()),
                "Tribunal de commerce de Nanterre".into()
            ))
        );
        assert_eq!(
            court("Tribunal administratif de Paris 1ère sec. 3e ch., décision du 6 mai 2024"),
            Some(("TA".into(), None, "Tribunal administratif de Paris".into()))
        );
        assert_eq!(
            court("Cour de cassation, ch. criminelle, arrêt du 21 juin 2016"),
            Some(("CC".into(), None, "Cour de cassation".into()))
        );
        assert_eq!(
            court("Centre d\u{2019}arbitrage et de médiation de l\u{2019}OMPI, décision du 2 mai 2012"),
            None
        );
    }

    #[test]
    fn a_first_instance_title_names_the_court_the_referential_holds() {
        let read = |title: &str| {
            let date = legalis_date(title).unwrap();
            legalis_court(title, &date).map(|c| (c.location.unwrap(), c.bench))
        };
        for (title, location, bench) in [
            ("Tribunal correctionnel de Béthune, Chambre collégiale, jugement correctionnel du 16 septembre 2015", "tgi_bethune", Some(CORRECTIONAL)),
            ("Tribunal de grande instance de Rennes Tribunal correctionnel Jugement du 6 décembre 2007", "tgi_rennes", Some(CORRECTIONAL)),
            ("Tribunal pour enfants de Béthune, jugement du 17 mai 2018", "tgi_bethune", Some("juge des enfants")),
            ("Tribunal d\u{2019}instance de Cherbourg Jugement du 12 juillet 2007", "ti_cherbourg", None),
            ("Tribunal d\u{2019}instance d\u{2019}Aix en Provence Juridiction de proximité Jugement du 9 janvier 2012", "prox_aix_en_provence", None),
            ("Juridiction de proximité  2ème arrondissement de Paris Jugement du 30 avril 2013", "prox_paris_2e_arrondissement", None),
            ("Tribunal d\u{2019}instance du 2ème arr. de Paris, jugement du 7 septembre 2015", "ti_paris_2e_arrondissement", None),
            ("Tribunal d\u{2019}instance de Paris XXème arr., jugement du 31 mars 2017", "ti_paris_20e_arrondissement", None),
            ("TI de Paris 6ème, jugement du 6 février 2018", "ti_paris_6e_arrondissement", None),
            ("Tribunal d\u{2019}instance de St Germain en Laye Jugement du 20 novembre 2008", "ti_saint_germain_en_laye", None),
            ("Tribunal d\u{2019}Instance de Saint Affrique (Aveyron) Jugement du 18 janvier 2001", "ti_saint_affrique", None),
            ("Tribunal de grande instance du Mans Jugement correctionnel 25 novembre 2002", "tgi_le_mans", None),
            ("Tribunal de grande instance de Dijon Quatrième chambre Jugement du 11 juillet 2006", "tgi_dijon", None),
            ("Tribunal de commerce de Paris Affaires contentieuses, 2ème chambre Jugement du 18 février 2003", "tcom_paris", None),
            ("Tribunal judicaire de Paris, ordonnance de référé, 14 janvier 2022", "tj_paris", None),
            ("Tribunal de première instance de Nouméa, ordonnance de référé du 7 février 2020", "tpi_noumea", None),
        ] {
            assert_eq!(read(title), Some((location.to_string(), bench)), "{title}");
        }
        for title in [
            "Tribunal de grande instance de Rennes Tribunal d\u{2019}instance Jugement du 26 mars 2007",
            "Cour d\u{2019}appel d\u{2019}Amiens Tribunal pour enfant 22 février 2005",
            "Tribunal d\u{2019}instance de Bordeaux Juridiction de août proximité Jugement du 18 août 2008",
            "Juridiction de proximité 3ème arrondissement Jugement du 25 juin 2013",
            "Tribunal de grande instance  17ème chambre correctionnel Jugement du 7 juillet 2009",
            "Tribunal d\u{2019}instance de police de Paris, 2ème chambre, 4 décembre 2003",
        ] {
            assert_eq!(read(title), None, "{title}");
        }
        assert_eq!(
            court("Tribunal correctionnel de Nanterre Jugement du 10 novembre 2011").map(|c| c.2),
            Some("Tribunal de grande instance de Nanterre".into())
        );
    }

    #[test]
    fn a_correctional_judgment_comes_from_the_court_of_its_year() {
        let location = |title: &str| court(title).and_then(|c| c.1);
        assert_eq!(
            location("Tribunal correctionnel de Lyon, jugement du 12 juin 2018").as_deref(),
            Some("tgi_lyon")
        );
        assert_eq!(
            location("Tribunal correctionnel de Lyon, jugement du 1er février 2021").as_deref(),
            Some("tj_lyon")
        );
    }

    #[test]
    fn a_title_dates_its_decision_and_not_the_one_it_mentions() {
        assert_eq!(
            legalis_date("Tribunal de grande instance de Strasbourg, ordonnance de référé du 3 février 1998 (infirmée par la cour d\u{2019}appel de Colmar le 9 septembre 1998)").as_deref(),
            Some("1998-02-03")
        );
        assert_eq!(
            legalis_date(
                "Tribunal de commerce de Paris, ordonnance de référé prononcée le 1er juillet 2024"
            )
            .as_deref(),
            Some("2024-07-01")
        );
        assert_eq!(legalis_date("Analyse statistique des décisions"), None);
    }

    #[test]
    fn the_lines_legalis_writes_under_the_decision_leave_the_text() {
        let body = "Le tribunal statue sur la demande de retrait d'un contenu publié en ligne par la société défenderesse, qui conteste sa qualité d'éditeur.\nPar ces motifs, le tribunal ordonne le retrait du contenu litigieux sous astreinte et condamne la défenderesse aux dépens.\nLe tribunal : A. Martin (président)\nAvocats : Me Paul Durand";
        let page = LegalisPage {
            post_id: 31513,
            title: "Cour d\u{2019}appel de Paris, Pôle 5 \u{2013} Ch. 11, arrêt du 10 avril 2026"
                .into(),
            text: format!("{body}\nSource : Legalis.net\nLire notre présentation de la décision"),
        };
        let parsed = legalis_decision(&page).unwrap();
        let d = &parsed.decision;
        assert_eq!(d.texte_integral_clean, body);
        assert_eq!(parsed.source_fields["origin"], "Legalis.net");
        assert_eq!(d.formation.as_deref(), Some("Pôle 5 \u{2013} Ch. 11"));
        assert_eq!(d.date_lecture.as_deref(), Some("2026-04-10"));
        assert_eq!(d.jurisdiction_location.as_deref(), Some("ca_paris"));
        assert_eq!(d.source_uid, "legalis/31513");
        let rebuilt = from_source_fields_legalis(
            &d.texte_integral_clean,
            &parsed.source_fields,
            &d.source_uid,
        );
        assert_eq!(&rebuilt, d);
    }
}
