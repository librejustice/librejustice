use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Sources européennes : CEDH (HUDOC) et CJUE (EUR-Lex).
//
// Parsers PURS : reçoivent le corps déjà strippé (HTML→texte au bord lj-sources)
// + les métadonnées désérialisées (`columns` HUDOC / `predicates` CDM). Le texte
// vit dans `texte_integral_*` ; les métadonnées partent verbatim en
// `source_fields` (construites par le pipeline depuis le même `Value`, hors de
// `Decision`).
// ─────────────────────────────────────────────────────────────────────────────

/// Lit une chaîne non-vide d'une clé d'un objet JSON (`columns`/`predicates`).
fn json_str_nonempty<'a>(obj: &'a Value, key: &str) -> Option<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Section markers of the European drafting, French and English, kept out of the
/// French set so that neither vocabulary can fire on the other.
static EUROPE_SECTION_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "procedure",
            Regex::new(
                r"(?m)^[ \t]*(?:PROCÉDURE|PROCEDURE|Procédure et conclusions des parties)[ \t]*$",
            )
                .unwrap(),
        ),
        (
            "expose",
            Regex::new(
                r"(?m)^[ \t]*(?:EN FAIT|THE FACTS|SUBJECT MATTER OF THE CASE|OBJET DE L['’]AFFAIRE|Antécédents du litige|Les faits à l['’]origine du litige|Le cadre juridique|Les antécédents du litige|Le litige au principal(?: et (?:la|les) questions? préjudicielles?)?)[ \t]*$",
            )
            .unwrap(),
        ),
        (
            "motivations",
            Regex::new(r"(?mi)^[ \t]*EN DROIT[ \t]*$").unwrap(),
        ),
        (
            "motivations",
            Regex::new(r"(?m)^[ \t]*(?:(?:AS TO )?THE LAW|THE COURT['’]S ASSESSMENT|APPRÉCIATION DE LA COUR)[ \t]*$").unwrap(),
        ),
        // The advocates general head their opinions in roman numerals; a line
        // ending on a page number belongs to the table of contents of the old ones.
        (
            "expose",
            Regex::new(
                r"(?m)^[ \t]*[IVX]+[ \t]*[.\x{2013}\x{2014}-][ \t]*(?:Le cadre|Le contexte|Les? faits?|Le litige|Les antécédents|La procédure|Les questions? préjudicielles?|Le droit applicable)(?:[^\n]{0,100}?[^\d\n])?[ \t]*$",
            )
            .unwrap(),
        ),
        (
            "motivations",
            Regex::new(
                r"(?m)^[ \t]*[IVX]+[ \t]*[.\x{2013}\x{2014}-][ \t]*(?:Analyse|Appréciation|Examen|Discussion|En droit|Réponses? aux questions|Sur (?:les|la) questions?|Sur le pourvoi|Sur le recours|Sur la recevabilité|Sur le fond)(?:[^\n]{0,100}?[^\d\n])?[ \t]*$",
            )
            .unwrap(),
        ),
        (
            "motivations",
            Regex::new(
                r"(?m)^[ \t]*(?:Sur (?:les|la) questions? préjudicielles?|Sur la demande de décision préjudicielle|Sur le pourvoi|Sur le recours|Sur la recevabilité[^\n]{0,60}|Sur le fond)[ \t]*$",
            )
            .unwrap(),
        ),
        (
            "motivations",
            Regex::new(r"(?m)^[ \t]*(?:ALLEGED VIOLATIONS? OF|SUR (?:LA|LES) VIOLATIONS? ALLÉGUÉES? DE)\b").unwrap(),
        ),
        // EUR-Lex layout, where the heading is also a table-of-contents entry at
        // the head of the document: in a CJUE judgment the title appears either
        // zero or exactly two times, and the trailing long line tells the real
        // one, with no hit on CEDH nor on advocate general opinions.
        (
            "motivations",
            Regex::new(
                r"(?m)^[ \t]*(Motifs de l['’]arrêt)[ \t]*\n(?:[ \t]*\n|[^\n]{0,60}\n)?[^\n]{40,}",
            )
            .unwrap(),
        ),
        (
            "dispositif",
            Regex::new(r"(?mi)^[ \t]*PAR CES MOTIFS").unwrap(),
        ),
        // The reasoning itself opens paragraphs on « For these reasons, the Court
        // finds »: the mixed case is the operative part only when the vote follows.
        (
            "dispositif",
            Regex::new(
                r"(?m)^[ \t]*(?:FOR THESE REASONS|For these reasons, the Court,? (?:unanimously|by a majority|by \w+ votes))",
            )
            .unwrap(),
        ),
    ]
});

/// A heading line opening on `words`, numbered in roman figures or not. No
/// closing punctuation, no page number of a contents line.
fn heading(kind: &'static str, words: &str) -> (&'static str, Regex) {
    let re = format!(
        r"(?m)^[ \t]*(?:[IVX]+[ \t]*[.\x{{2013}}\x{{2014}}-]+[ \t]*)?(?:{words})\b(?:[^\n]{{0,80}}?[^\d\n.:;,])?[ \t]*$"
    );
    (kind, Regex::new(&re).unwrap())
}

/// The headings advocates general write unnumbered, or under other words than
/// the judgments'.
static OPINION_SECTION_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let mut patterns = EUROPE_SECTION_PATTERNS.clone();
    patterns.extend([
        heading(
            "expose",
            r"Introduction|(?:L['’])?[Oo]bjet du litige|(?:Le )?[Cc]adre (?:juridique|normatif|réglementaire)|(?:La|Les) (?:législation|réglementation|dispositions)|Le droit (?:de l['’]Union|communautaire|national|applicable)|(?:Les )?[Ff]aits|(?:Les )?[Aa]ntécédents",
        ),
        heading("procedure", r"(?:La )?[Pp]rocédure"),
        heading(
            "moyens",
            r"(?:Les )?(?:[Mm]oyens et )?[Aa]rguments des parties|(?:Bref )?[Rr]ésumé des (?:principaux )?arguments|Observations (?:des parties|présentées|soumises)",
        ),
        heading(
            "motivations",
            r"Analyse|Appréciation|Examen|Discussion|(?:Sur la )?[Rr]ecevabilité|(?:Au|Sur le) fond|(?:Observations|Remarques) (?:liminaires|préliminaires)",
        ),
    ]);
    patterns
});

type Markers = Vec<(&'static str, Regex)>;

/// The CJUE in the languages it is shown in when French is missing: the
/// headings of its judgments, orders and opinions, the formula its operative
/// part opens on, and the heading over it, which a contents line also reads.
static CJUE_FOREIGN_PATTERNS: LazyLock<Vec<(&'static str, Markers)>> = LazyLock::new(|| {
    let set = |headings: [(&'static str, &str); 4], operative: &str, over: &str| {
        let mut patterns: Vec<(&'static str, Regex)> = headings
            .into_iter()
            .map(|(kind, words)| heading(kind, words))
            .collect();
        patterns.push((
            "dispositif",
            Regex::new(&format!(r"(?m)^[ \t]*(?:{operative})\b")).unwrap(),
        ));
        patterns.push((
            "dispositif",
            Regex::new(&format!(r"(?m)^[^\n]{{40,}}\n[ \t]*({over})[ \t]*$")).unwrap(),
        ));
        patterns
    };
    vec![
        (
            "en",
            set(
                [
                    ("procedure", r"Procedure|Proceedings"),
                    (
                        "expose",
                        r"Background to the dispute|Legal (?:context|framework)|(?:The )?[Ff]acts|The dispute in the main proceedings|Introduction",
                    ),
                    ("moyens", r"Arguments of the parties|Forms of order sought"),
                    (
                        "motivations",
                        r"Assessment of the Court|Findings of the Court|Law|Admissibility|Substance|The appeal|Consideration of the questions? referred|Analysis|Assessment",
                    ),
                ],
                r"On those grounds",
                "Operative part",
            ),
        ),
        (
            "it",
            set(
                [
                    ("procedure", r"Procedimento"),
                    (
                        "expose",
                        r"Fatti|Antefatti|Contesto normativo|Quadro (?:normativo|giuridico)|Procedimento principale e questioni? pregiudizial[ei]",
                    ),
                    ("moyens", r"Argomenti delle parti"),
                    (
                        "motivations",
                        r"In diritto|Sull['’]impugnazione|Sulla ricevibilità|Nel merito|Giudizio del Tribunale|Valutazione del Tribunale|Sulle questioni pregiudiziali|Analisi",
                    ),
                ],
                r"Per questi motivi",
                "Dispositivo",
            ),
        ),
        (
            "de",
            set(
                [
                    ("procedure", r"Verfahren|Anträge der Parteien"),
                    (
                        "expose",
                        r"Sachverhalt|Vorgeschichte des Rechtsstreits|Rechtlicher Rahmen|Rechtsrahmen|Ausgangsverfahren",
                    ),
                    ("moyens", r"Vorbringen der Parteien"),
                    (
                        "motivations",
                        r"Rechtliche Würdigung|Würdigung durch (?:das Gericht|den Gerichtshof)|Zum Rechtsmittel|Zur Zulässigkeit|Zur Begründetheit|Zu den Vorlagefragen",
                    ),
                ],
                r"Aus diesen Gründen",
                "Tenor",
            ),
        ),
        (
            "es",
            set(
                [
                    ("procedure", r"Procedimiento|Pretensiones de las partes"),
                    (
                        "expose",
                        r"Hechos|Antecedentes del litigio|Marco jurídico|Litigio principal",
                    ),
                    (
                        "moyens",
                        r"(?:Resumen de las )?[Aa]legaciones de las partes",
                    ),
                    (
                        "motivations",
                        r"Fundamentos de Derecho|Sobre la admisibilidad|Sobre el fondo|Apreciación del Tribunal|Sobre el recurso de casación|Sobre las cuestiones prejudiciales|Análisis",
                    ),
                ],
                r"En virtud de todo lo expuesto",
                "Parte dispositiva",
            ),
        ),
    ]
});

pub(crate) fn extract_sections_europe(cleaned_text: &str) -> Vec<DecisionSection> {
    sections_from_markers(cleaned_text, &EUROPE_SECTION_PATTERNS, false)
}

/// The sections of a CJUE text in `lang`. The reasoning runs to the operative
/// part: a heading of facts, procedure or arguments inside it heads a part of it.
fn cjue_sections(cleaned_text: &str, celex: &str, lang: &str) -> Vec<DecisionSection> {
    let patterns: &[(&'static str, Regex)] =
        match CJUE_FOREIGN_PATTERNS.iter().find(|(l, _)| *l == lang) {
            Some((_, patterns)) => patterns,
            None if celex_is_opinion(celex) => &OPINION_SECTION_PATTERNS,
            None => &EUROPE_SECTION_PATTERNS,
        };
    let mut out: Vec<DecisionSection> = Vec::new();
    let mut reasoning = false;
    for section in sections_from_markers(cleaned_text, patterns, false) {
        match out.last_mut() {
            Some(previous) if reasoning && section.kind != "dispositif" => {
                previous.end_char = section.end_char;
                previous.text = char_slice(cleaned_text, previous.start_char, previous.end_char)
                    .trim()
                    .to_string();
            }
            _ => {
                reasoning |= section.kind == "motivations";
                out.push(section);
            }
        }
    }
    out
}

/// An advocate general's opinion or view: `CC`/`CP` in the CELEX, a bare `C`
/// before the number in the older ones (`61990C0090`).
fn celex_is_opinion(celex: &str) -> bool {
    let b = celex.as_bytes();
    b.get(5) == Some(&b'C') && matches!(b.get(6), Some(b'C' | b'P' | b'0'..=b'9'))
}

/// Values of a CDM predicate, whose builder emits a scalar on a single value and
/// an array beyond (`lj_sources::cjue::map_predicates`).
fn json_strs<'a>(obj: &'a Value, key: &str) -> Vec<&'a str> {
    match obj.get(key) {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(vals)) => vals.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// `kpdate` HUDOC (`1960-11-14T00:00:00`, ISO horodaté) → `YYYY-MM-DD`. On retient
/// les 10 premiers caractères ssi ce sont une date ISO valide (`%Y-%m-%d`) — pas
/// de troncature aveugle. `None` sinon.
fn cedh_kpdate_to_iso(kpdate: &str) -> Option<String> {
    let head = kpdate.get(..10)?;
    jiff::civil::Date::strptime("%Y-%m-%d", head)
        .ok()
        .map(|_| head.to_string())
}

/// `judgementdate` HUDOC (`14/11/1960 00:00:00`, `DD/MM/YYYY` + heure) → `YYYY-MM-DD`.
/// Repli quand `kpdate` manque. `None` si le préfixe n'est pas une date FR valide.
fn cedh_judgementdate_to_iso(judgementdate: &str) -> Option<String> {
    let head = judgementdate.split_whitespace().next()?;
    let d = jiff::civil::Date::strptime("%d/%m/%Y", head).ok()?;
    Some(d.strftime("%Y-%m-%d").to_string())
}

/// Parse un arrêt CEDH (HUDOC) en [`Decision`]. `body_text` est le corps déjà
/// converti HTML→texte au bord (`lj-sources`) ; `columns` est le bloc
/// `results[].columns` désérialisé ; `itemid` est la PK HUDOC (`001-…`).
///
/// `jurisdiction_type` = `"CEDH"` (posé explicitement). `ecli` lu verbatim de
/// `columns["ecli"]` (arrêts seulement, `None` sinon) — jamais dérivé.
/// `numero_dossiers` = `columns["appno"]` (`;`-séparé → liste). `date_lecture` =
/// `columns["kpdate"]` normalisé ISO `YYYY-MM-DD` (repli `judgementdate`
/// `DD/MM/YYYY`), car la frontière store ne parse que `%Y-%m-%d`. Le titre
/// (`docname`) et toutes les autres colonnes restent en `source_fields`.
///
/// Erreur franche [`CoreError::Xml`] si `itemid` ou `body_text` est vide.
pub fn parse_cedh(
    body_text: &str,
    columns: &Value,
    itemid: &str,
) -> lj_core::error::Result<Decision> {
    use lj_core::error::CoreError;

    if itemid.is_empty() {
        return Err(CoreError::Xml("CEDH: itemid vide".to_string()));
    }
    if body_text.trim().is_empty() {
        return Err(CoreError::Xml(format!("CEDH {itemid}: corps vide")));
    }

    let sections = extract_sections_europe(body_text);

    let numero_dossiers = json_str_nonempty(columns, "appno").map(|appno| {
        appno
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let numero_dossier = numero_dossiers.as_ref().and_then(|v| v.first().cloned());

    // `date_lecture` sort en ISO `YYYY-MM-DD` : la frontière store ne parse QUE
    // `%Y-%m-%d`. HUDOC livre `kpdate` en ISO horodaté (`1960-11-14T00:00:00`) et
    // `judgementdate` en `DD/MM/YYYY HH:MM:SS` — kpdate prioritaire.
    let date_lecture = json_str_nonempty(columns, "kpdate")
        .and_then(cedh_kpdate_to_iso)
        .or_else(|| {
            json_str_nonempty(columns, "judgementdate").and_then(cedh_judgementdate_to_iso)
        });

    Ok(Decision {
        source_uid: format!("cedh/{itemid}"),
        member_name: itemid.to_string(),
        ecli: json_str_nonempty(columns, "ecli").map(str::to_string),
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some("Cour européenne des droits de l'homme".to_string()),
        jurisdiction_type: Some("CEDH".to_string()),
        jurisdiction_location: None,
        numero_dossier,
        numero_dossiers,
        numero_role: None,
        date_lecture,
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        texte_integral_raw: body_text.to_string(),
        texte_integral_clean: body_text.to_string(),
        sections,
        metadata_header: String::new(),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    })
}

/// `(number, two-digit year)` of a sector 6 CELEX (`61995CJ0030` → `("30",
/// "95")`), which encodes sector `6`, the four-digit case year, the type
/// (`CJ`/`TJ`/`CO`/`TO`/`CC`…) then the number, leading zeros included.
fn celex_number_year(celex: &str) -> Option<(String, String)> {
    let bytes = celex.as_bytes();
    if bytes.len() < 7 || bytes[0] != b'6' || !bytes[1..5].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let rest = &celex[5..];
    let type_end = rest.find(|c: char| c.is_ascii_digit()).filter(|&i| i > 0)?;
    let digits: String = rest[type_end..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let number = digits.trim_start_matches('0');
    let number = if number.is_empty() { "0" } else { number };
    Some((number.to_string(), celex[3..5].to_string()))
}

/// Court letter, initial of the CELEX type (`62008TJ0133` → `T`).
fn celex_court_letter(celex: &str) -> Option<char> {
    celex.chars().nth(5).filter(char::is_ascii_uppercase)
}

/// Usual case number of a CELEX (`62020CJ0560` → `C-560/20`), the raw CELEX when
/// the shape is not recognised.
pub fn celex_to_case_number(celex: &str) -> String {
    match (celex_court_letter(celex), celex_number_year(celex)) {
        (Some(court), Some((number, yy))) => format!("{court}-{number}/{yy}"),
        _ => celex.to_string(),
    }
}

/// `(number, year)` of a case number already written out (`C-125/13` → `("125",
/// "13")`). The year has to hold on two digits: CELLAR writes `T-50/6` where the
/// case is from 2006, and that identifier names nothing.
fn ident_number_year(ident: &str) -> Option<(String, String)> {
    let (head, year) = ident.rsplit_once('/')?;
    let number = head.rsplit_once('-').map_or(head, |(_, n)| n);
    let named = year.len() == 2
        && year.bytes().all(|b| b.is_ascii_digit())
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit());
    named.then(|| (number.to_string(), year.to_string()))
}

/// Opens a CELLAR case URI, under both its spellings:
/// `…/resource/case/celex%3A61995CJ0030` and `…/resource/case/C-125%2F13`.
const JOINED_CASE_URI: &str = "/resource/case/";

/// Cases the judgment settles alongside its own, read from the CDM predicate
/// `case-law_joins_case_court`. CELLAR names them three ways and only the number
/// and the year hold across the three: the court letter of the CELEX URI says
/// `CJ` where the work is a `TJ` at times, so it comes from the work that judges. A
/// cellar UUID that `dossier_identifier` did not resolve, a one-digit year or a label
/// name no case.
fn joined_case_numbers(predicates: &Value, celex: &str) -> Vec<String> {
    let Some(court) = celex_court_letter(celex) else {
        return Vec::new();
    };
    json_strs(predicates, "case-law_joins_case_court")
        .into_iter()
        .filter_map(|raw| {
            let (number, year) = match raw.rsplit_once(JOINED_CASE_URI) {
                Some((_, tail)) => match tail.strip_prefix("celex%3A") {
                    Some(joined) => celex_number_year(joined)?,
                    None => ident_number_year(&tail.replace("%2F", "/"))?,
                },
                None if raw.contains("/resource/") => return None,
                None => ident_number_year(raw)?,
            };
            Some(format!("{court}-{number}/{year}"))
        })
        .collect()
}

/// Parses a CJUE judgment (EUR-Lex) into a [`Decision`]. `body_text` is the text
/// in `lang` (ISO 639-1) already converted (xhtml/html to text) at the edge
/// (`lj-sources`), `predicates` the deserialised CDM predicate block, `celex`
/// the PK.
///
/// `ecli` is read verbatim from `predicates["case-law_ecli"]`, **never derived
/// from the CELEX**, which would be false. `numero_dossiers` is the case number
/// of the CELEX then [`joined_case_numbers`], the raw CELEX as fallback. The
/// « objet » banner of the header and the other predicates stay in
/// `source_fields`.
///
/// Hard [`CoreError::Xml`] when `celex` or `body_text` is empty.
pub fn parse_cjue(
    body_text: &str,
    predicates: &Value,
    celex: &str,
    lang: &str,
) -> lj_core::error::Result<Decision> {
    use lj_core::error::CoreError;

    if celex.is_empty() {
        return Err(CoreError::Xml("CJUE: celex vide".to_string()));
    }
    if body_text.trim().is_empty() {
        return Err(CoreError::Xml(format!("CJUE {celex}: corps vide")));
    }

    let sections = cjue_sections(body_text, celex, lang);

    let case_number = celex_to_case_number(celex);
    let mut dossiers = vec![case_number.clone()];
    for joined in joined_case_numbers(predicates, celex) {
        if !dossiers.contains(&joined) {
            dossiers.push(joined);
        }
    }

    Ok(Decision {
        source_uid: format!("cjue/{celex}/{lang}"),
        member_name: celex.to_string(),
        ecli: json_str_nonempty(predicates, "case-law_ecli").map(str::to_string),
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some("Cour de justice de l'Union européenne".to_string()),
        jurisdiction_type: Some("CJUE".to_string()),
        jurisdiction_location: None,
        numero_dossier: Some(case_number),
        numero_dossiers: Some(dossiers),
        numero_role: None,
        date_lecture: json_str_nonempty(predicates, "work_date_document").map(str::to_string),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        texte_integral_raw: body_text.to_string(),
        texte_integral_clean: body_text.to_string(),
        sections,
        metadata_header: String::new(),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    })
}

/// Mots qui lient les parties sans en nommer aucune.
const LINK_WORDS: [&str; 4] = ["et", "autres", "and", "others"];

/// Séparateurs des deux parties d'un nom d'affaire : « c. » à Strasbourg,
/// « contre » à Luxembourg, « v. » quand la source ne livre que l'anglais.
const PARTY_SEPARATORS: [&str; 4] = ["c.", "v.", "v", "contre"];

/// `true` si le mot n'est qu'une suite d'initiales pointées (« N.T.P. »,
/// « M.T. »), ce que la pseudonymisation laisse d'un patronyme.
fn is_initials(word: &str) -> bool {
    word.ends_with('.')
        && word
            .split_terminator('.')
            .all(|part| part.chars().count() == 1)
}

/// Le nom désigne-t-il quelqu'un ? Il s'écrit « <requérant> <séparateur>
/// <défendeur> », et ne vaut que si la partie qui précède le séparateur porte un
/// mot : « AFFAIRE K c. FRANCE » nomme l'anonymat que la Cour a accordé.
fn names_a_party(case_name: &str) -> bool {
    let words: Vec<&str> = case_name.split_whitespace().collect();
    let separator = words
        .iter()
        .position(|w| PARTY_SEPARATORS.contains(&w.to_lowercase().as_str()));
    let Some(separator) = separator else {
        return false;
    };
    words[..separator].iter().any(|w| {
        !LINK_WORDS.contains(&w.to_lowercase().as_str())
            && !is_initials(w)
            && w.chars().filter(|c| c.is_alphabetic()).count() >= 2
    })
}

/// Étiquettes que HUDOC pose devant le nom de l'affaire.
const HUDOC_LABELS: [&str; 4] = ["AFFAIRES ", "AFFAIRE ", "CASE OF ", "ARRET "];

/// Le nom sous lequel la CEDH désigne sa propre affaire, lu du `docname`
/// HUDOC : « AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE » → « TAMAKULOVA ET AUTRES
/// c. UKRAINE ». La casse de la source est gardée telle quelle : l'abaisser
/// détruirait le `İ` turc, qui vaut deux caractères en minuscule. `None` quand
/// le docname ne nomme personne.
pub fn cedh_case_name(docname: &str) -> Option<String> {
    let docname = lj_core::text::normalize_spaces(docname);
    // « [Extraits] » dit ce que le recueil publie, pas qui est en cause.
    let without_mention = match (docname.ends_with(']'), docname.rfind('[')) {
        (true, Some(open)) => docname[..open].trim_end(),
        _ => docname.as_str(),
    };
    let name = HUDOC_LABELS
        .iter()
        .find_map(|label| without_mention.strip_prefix(label))
        .unwrap_or(without_mention);
    names_a_party(name).then(|| name.to_string())
}

/// Étiquettes ouvrant la ligne de titre EUR-Lex.
const EURLEX_LABELS: [&str; 4] = ["Arrêt ", "Ordonnance ", "Conclusions ", "Judgment "];

/// Séparateur des champs de la ligne de titre EUR-Lex.
const EURLEX_FIELD: &str = ". - ";

/// Lignes de tête où chercher la ligne de titre (le bandeau EUR-Lex en occupe
/// quatre).
const EURLEX_HEAD_LINES: usize = 12;

/// Le nom de l'affaire tel qu'EUR-Lex l'écrit en tête du texte. La ligne de
/// titre aligne « <acte> du <date>. - <parties>. - <objet>. - <affaire> » : le
/// nom est le champ des parties. Les entités HTML que la source laisse passer y
/// sont dépliées (`M&uuml;nchen`). `None` hors de cette forme : les textes
/// modernes n'ont pas de ligne de titre, et ne nomment leurs parties que dans le
/// corps, où rien ne les délimite.
pub fn cjue_case_name(full_text: &str) -> Option<String> {
    let title = full_text
        .lines()
        .take(EURLEX_HEAD_LINES)
        .map(str::trim)
        .find(|line| {
            line.contains(EURLEX_FIELD) && EURLEX_LABELS.iter().any(|l| line.starts_with(l))
        })?;
    let parties = title.split(EURLEX_FIELD).nth(1)?;
    let name = html_escape::decode_html_entities(parties)
        .trim()
        .to_string();
    names_a_party(&name).then_some(name)
}

/// The parties a CELLAR French title names in its second `#` field, the final full
/// stop dropped unless it ends an abbreviation (« e.a. », « S.A. »).
pub fn cjue_title_case_name(title: &str) -> Option<String> {
    let parties = title.split('#').nth(1)?.trim();
    let last = parties.rsplit(' ').next().unwrap_or_default();
    let name = match parties.strip_suffix('.') {
        Some(name) if !last[..last.len() - 1].contains('.') => name,
        _ => parties,
    };
    names_a_party(name).then(|| name.to_string())
}

/// The heading a regulation or a directive opens on, whatever its case and accents.
static ACT_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:r[eéè]glement|directive)\b").expect("act heading regex")
});

/// A CELLAR French act from its heading on: EUR-Lex's notice and the OJ page head dropped.
pub fn eurlex_act_text(text: &str) -> &str {
    let lines: Vec<&str> = text.split_inclusive('\n').take(EURLEX_HEAD_LINES).collect();
    let notice = lines
        .iter()
        .any(|l| l.trim() == "Avis juridique important")
        .then(|| {
            let stars = |l: &&str| l.trim().len() >= 3 && l.trim().bytes().all(|b| b == b'*');
            lines.iter().position(stars).or_else(|| {
                lines
                    .iter()
                    .position(|l| l.trim_start().starts_with("Journal officiel n"))
            })
        })
        .flatten()
        .map_or(0, |i| i + 1);
    let start = lines[notice..]
        .iter()
        .position(|l| ACT_HEADING.is_match(l))
        .map_or(notice, |i| notice + i);
    &text[lines[..start].iter().map(|l| l.len()).sum::<usize>()..]
}

/// `true` si `source_uid` provient d'une source HTML européenne : préfixe
/// `cedh/` (HUDOC) ou `cjue/` (EUR-Lex), posé par [`parse_cedh`] /
/// [`parse_cjue`]. Discriminant de famille du dispatch
/// [`Decision::from_source_fields`] — ces préfixes sont stables en DB et jamais
/// portés par les autres fonds (XML opendata, JSON Judilibre, DILA, CNDA).
pub(crate) fn source_uid_is_html_europe(source_uid: &str) -> bool {
    source_uid.starts_with("cedh/") || source_uid.starts_with("cjue/")
}

/// European HTML branch (CEDH/CJUE) of [`Decision::from_source_fields`]:
/// rebuilds a `Decision` **identical** to [`parse_cedh`]/[`parse_cjue`] from
/// `(full_text, source_fields)`. Exact counterpart of [`build_source_fields`] on
/// the HTML side: `source_fields` = the metadata (HUDOC columns / CDM
/// predicates) verbatim + the `sections` key (which the parsers ignore, reading
/// the sections again from `full_text`).
///
/// The parity is structural: these parsers set `texte_integral_clean =
/// texte_integral_raw = body_text` (no cleaning) and `metadata_header = ""`, so
/// the chunk depends on `full_text` alone (stored verbatim).
/// The dedicated parser runs again on `(full_text, source_fields, pk)`, `pk`
/// (`itemid`/`celex`) derived from the `source_uid` (`cedh/<itemid>` /
/// `cjue/<celex>/<lang>`).
///
/// Panics if the parser fails: downstream, `(full_text, source_fields)` comes
/// from a decision already ingested, hence valid.
pub(crate) fn from_source_fields_html_europe(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    if let Some(itemid) = source_uid.strip_prefix("cedh/") {
        parse_cedh(full_text, source_fields, itemid)
            .unwrap_or_else(|e| panic!("from_source_fields CEDH {source_uid}: {e}"))
    } else if let Some((celex, lang)) = source_uid
        .strip_prefix("cjue/")
        .and_then(|rest| rest.split_once('/'))
    {
        parse_cjue(full_text, source_fields, celex, lang)
            .unwrap_or_else(|e| panic!("from_source_fields CJUE {source_uid}: {e}"))
    } else {
        panic!("from_source_fields_html_europe: source_uid is not European HTML {source_uid}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── CEDH / CJUE ────────────────────────────────────────────────────────

    /// Each section's kind and first line.
    fn kinds_at(text: &str) -> Vec<(String, String)> {
        extract_sections_europe(text)
            .into_iter()
            .map(|s| {
                let head = s.text.lines().next().unwrap_or("").to_string();
                (s.kind, head)
            })
            .collect()
    }

    #[test]
    fn english_judgment_opens_its_operative_part_on_the_vote_only() {
        let text = "CASE OF X v. POLAND\nPROCEDURE\n1. The case originated in an application.\nTHE FACTS\n2. The applicant was born in 1980.\nTHE LAW\n3. The applicant complained under Article 5.\nFor these reasons, the Court finds that there has been a violation of Article 5 § 3.\nFOR THESE REASONS, THE COURT, UNANIMOUSLY,\nDeclares the application admissible;";
        let kinds = kinds_at(text);
        let names: Vec<&str> = kinds.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "preamble",
                "procedure",
                "expose",
                "motivations",
                "dispositif"
            ]
        );
        assert_eq!(kinds[4].1, "FOR THESE REASONS, THE COURT, UNANIMOUSLY,");
    }

    #[test]
    fn advocate_general_headings_skip_the_table_of_contents() {
        let text = "CONCLUSIONS DE L'AVOCAT GÉNÉRAL\nSOMMAIRE\nII \u{2014} Le cadre juridique 3-5\nIV \u{2014} Analyse 6-9\nI. Introduction\n1. La présente affaire porte sur la directive.\nII. Le cadre juridique\n2. L'article 1er de la directive dispose.\nIII. Les faits au principal et la question préjudicielle\n3. Le requérant a saisi la juridiction de renvoi.\nIV. Analyse\n4. La question porte sur l'article 2.\nV. Conclusion\n5. Je propose à la Cour de répondre comme suit.";
        let kinds = kinds_at(text);
        assert_eq!(
            kinds,
            [
                ("preamble", "CONCLUSIONS DE L'AVOCAT GÉNÉRAL"),
                ("expose", "II. Le cadre juridique"),
                ("motivations", "IV. Analyse"),
            ]
            .map(|(k, h)| (k.to_string(), h.to_string()))
        );
    }

    #[test]
    fn opinion_headings_need_no_number_and_late_arguments_stay_in_the_analysis() {
        let text = "Conclusions de l'avocat général\nLes dispositions pertinentes sont les suivantes :\nLa législation nationale applicable\n1. La loi dispose.\nLa procédure devant la Cour\n2. La Commission a introduit un recours.\nV -- Appréciation\n3. Le recours est fondé.\nObservations des parties\n4. L'Irlande conteste.\nConclusion\n5. Je propose à la Cour de condamner l'Irlande.";
        let sections = cjue_sections(text, "61997C0358", "fr");
        let heads: Vec<(&str, &str)> = sections
            .iter()
            .map(|s| (s.kind.as_str(), s.text.lines().next().unwrap_or("")))
            .collect();
        assert_eq!(
            heads,
            [
                ("preamble", "Conclusions de l'avocat général"),
                ("expose", "La législation nationale applicable"),
                ("procedure", "La procédure devant la Cour"),
                ("motivations", "V -- Appréciation"),
            ]
        );
        assert!(sections[3].text.contains("L'Irlande conteste"));
        assert_eq!(sections[3].end_char, char_len(text));

        assert!(celex_is_opinion("61990C0090"));
        assert!(celex_is_opinion("62010CC0342"));
        assert!(!celex_is_opinion("62004TJ0162"));
        assert!(!celex_is_opinion("62011CO0348"));
        assert!(!celex_is_opinion("61999B0073"));
    }

    #[test]
    fn a_foreign_judgment_is_cut_past_its_contents_and_keeps_its_reasoning_whole() {
        let heads = |text: &str, lang: &str| -> Vec<(String, String)> {
            cjue_sections(text, "61994TJ0140", lang)
                .into_iter()
                .map(|s| (s.kind, s.text.lines().next().unwrap_or("").to_string()))
                .collect()
        };
        let spanish = "SENTENCIA DEL TRIBUNAL DE PRIMERA INSTANCIA\nPartes\nMotivación de la sentencia\nDecisión sobre las costas\nParte dispositiva\nHechos\n1 El demandante es funcionario de la Comisión desde 1990.\nProcedimiento y pretensiones de las partes\n2 Mediante escrito presentado en la Secretaría del Tribunal.\nSobre el fondo\n3 Procede desestimar el recurso por infundado en su totalidad.\nParte dispositiva\nEn virtud de todo lo expuesto,\nEL TRIBUNAL decide:";
        assert_eq!(
            heads(spanish, "es"),
            [
                ("preamble", "SENTENCIA DEL TRIBUNAL DE PRIMERA INSTANCIA"),
                ("expose", "Hechos"),
                ("procedure", "Procedimiento y pretensiones de las partes"),
                ("motivations", "Sobre el fondo"),
                ("dispositif", "Parte dispositiva"),
            ]
            .map(|(k, h)| (k.to_string(), h.to_string()))
        );
        let italian = "ORDINANZA DEL TRIBUNALE\nFatti e procedimento in primo grado\n1 Il ricorrente è un dipendente.\nSull'impugnazione\nProcedimento e conclusioni delle parti\n2 Il ricorrente chiede l'annullamento.\n3 L'impugnazione è respinta.\nPer questi motivi,\nIL TRIBUNALE così provvede:";
        assert_eq!(
            heads(italian, "it"),
            [
                ("preamble", "ORDINANZA DEL TRIBUNALE"),
                ("expose", "Fatti e procedimento in primo grado"),
                ("motivations", "Sull'impugnazione"),
                ("dispositif", "Per questi motivi,"),
            ]
            .map(|(k, h)| (k.to_string(), h.to_string()))
        );
    }

    #[test]
    fn cedh_maps_fields_ecli_appno_list_and_type() {
        let columns = json!({
            "itemid": "001-250438",
            "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE",
            "appno": "20890/16;19185/25;26238/10",
            "ecli": "ECLI:CE:ECHR:2026:0611JUD002089016",
            "judgementdate": "2026-06-11T00:00:00",
            "kpdate": "2026-06-11T00:00:00",
            "doctype": "HFJUD",
        });
        let body = "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE\n\nPROCÉDURE\n\nÀ l'origine de l'affaire se trouve une requête dirigée contre l'Ukraine.\n\nEN FAIT\n\nLes circonstances de l'espèce.\n\nEN DROIT\n\nSur la violation alléguée de l'article 6 de la Convention.\n\nPAR CES MOTIFS, LA COUR, À L'UNANIMITÉ,\n\nDéclare la requête recevable.";
        let d = parse_cedh(body, &columns, "001-250438").expect("parse CEDH");

        assert_eq!(d.source_uid, "cedh/001-250438");
        assert_eq!(d.member_name, "001-250438");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CEDH"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Cour européenne des droits de l'homme")
        );
        assert_eq!(
            d.ecli.as_deref(),
            Some("ECLI:CE:ECHR:2026:0611JUD002089016")
        );
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(
                &[
                    "20890/16".to_string(),
                    "19185/25".to_string(),
                    "26238/10".to_string()
                ][..]
            )
        );
        assert_eq!(d.numero_dossier.as_deref(), Some("20890/16"));
        assert_eq!(d.date_lecture.as_deref(), Some("2026-06-11"));
        assert_eq!(d.texte_integral_clean, body);
        assert_eq!(d.texte_integral_raw, body);
        let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                "preamble",
                "procedure",
                "expose",
                "motivations",
                "dispositif"
            ]
        );
    }

    #[test]
    fn cedh_ecli_none_when_absent_and_kpdate_fallback() {
        let columns = json!({
            "appno": "10000/20",
            "ecli": "",
            "kpdate": "2026-05-01T00:00:00",
        });
        let d =
            parse_cedh("Résumé juridique du litige.", &columns, "002-14552").expect("parse CEDH");
        assert_eq!(d.ecli, None);
        assert_eq!(d.date_lecture.as_deref(), Some("2026-05-01"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["10000/20".to_string()][..])
        );
    }

    #[test]
    fn cedh_date_falls_back_to_judgementdate_ddmmyyyy_normalized_iso() {
        let columns = json!({
            "appno": "332/57",
            "ecli": "ECLI:CE:ECHR:1960:1114JUD000033257",
            "judgementdate": "14/11/1960 00:00:00",
        });
        let d = parse_cedh("Arrêt Lawless.", &columns, "001-57516").expect("parse CEDH");
        assert_eq!(d.date_lecture.as_deref(), Some("1960-11-14"));
    }

    #[test]
    fn cedh_errors_on_empty_itemid_or_body() {
        let columns = json!({ "appno": "1/20" });
        assert!(parse_cedh("texte", &columns, "").is_err());
        assert!(parse_cedh("   ", &columns, "001-1").is_err());
    }

    #[test]
    fn cjue_maps_fields_ecli_verbatim_and_case_number_from_celex() {
        let predicates = json!({
            "case-law_ecli": "ECLI:EU:C:2024:96",
            "work_date_document": "2024-01-30",
            "resource_legal_type": "CJ",
            "subject-matter": "IMMI",
        });
        let body = "ARRÊT DE LA COUR (grande chambre) 30 janvier 2024. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour dit pour droit.";
        let d = parse_cjue(body, &predicates, "62020CJ0560", "fr").expect("parse CJUE");

        assert_eq!(d.source_uid, "cjue/62020CJ0560/fr");
        assert_eq!(d.member_name, "62020CJ0560");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CJUE"));
        assert_eq!(d.ecli.as_deref(), Some("ECLI:EU:C:2024:96"));
        assert_ne!(d.ecli.as_deref(), Some("ECLI:EU:C:2020:560"));
        assert_eq!(d.numero_dossier.as_deref(), Some("C-560/20"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["C-560/20".to_string()][..])
        );
        assert_eq!(d.date_lecture.as_deref(), Some("2024-01-30"));
        assert_eq!(d.texte_integral_clean, body);
    }

    /// The EUR-Lex layout opens on a table of contents that repeats the headings,
    /// so the motifs boundary falls on the second one, the only one a real
    /// paragraph follows.
    #[test]
    fn cjue_motifs_skip_the_table_of_contents_heading() {
        let predicates = json!({ "work_date_document": "1993-07-02" });
        let body = "61991J0345\n\
                    Recueil de jurisprudence 1993 page I-04287\n\
                    Sommaire\n\
                    Parties\n\
                    Motifs de l'arrêt\n\
                    Décisions sur les dépenses\n\
                    Dispositif\n\
                    Mots clés\n\
                    Politique sociale - Travailleurs masculins et travailleurs féminins.\n\
                    Parties\n\
                    Dans l'affaire C-345/91, ayant pour objet une demande adressée à la Cour.\n\
                    rend le présent\n\
                    Arrêt\n\
                    Motifs de l'arrêt\n\
                    1 Par jugement du 22 mai 1991, parvenu à la Cour le 18 juin suivant, le tribunal.\n\
                    PAR CES MOTIFS, la Cour dit pour droit.";
        let d = parse_cjue(body, &predicates, "61991J0345", "fr").expect("parse CJUE");

        let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, vec!["preamble", "motivations", "dispositif"]);
        let motifs = &d.sections[1].text;
        assert!(
            motifs.starts_with("Motifs de l'arrêt\n1 Par jugement"),
            "{motifs}"
        );
    }

    /// Joined cases are the only way to reach a judgment by another number than
    /// its lead one, and CELLAR names them three ways in the same predicate. The
    /// test holds the three, plus the two that name nothing.
    #[test]
    fn cjue_joined_cases_join_the_docket_numbers_whatever_the_shape() {
        let predicates = json!({
            "case-law_ecli": "ECLI:EU:T:2010:1",
            "work_date_document": "2010-03-09",
            "case-law_joins_case_court": [
                // CELEX URI, whose court letter says `CJ` where the work is a `TJ`.
                "http://publications.europa.eu/resource/case/celex%3A62008CJ0134",
                // Identifier already written out, `/` encoded by CELLAR.
                "http://publications.europa.eu/resource/case/T-177%2F08",
                // `dossier_identifier`, resolved by the SPARQL hop.
                "T-242/09",
                // Cellar UUID the hop did not resolve, and a label.
                "http://publications.europa.eu/resource/cellar/b77842bc-7299-11f0-bf4e",
                "http://publications.europa.eu/resource/case/AFFAIRE",
                // One-digit year: CELLAR writes `T-50/6` for 2006.
                "http://publications.europa.eu/resource/case/T-50%2F6",
            ],
        });
        let d = parse_cjue(
            "ARRÊT DU TRIBUNAL. PAR CES MOTIFS.",
            &predicates,
            "62008TJ0133",
            "fr",
        )
        .expect("parse CJUE");

        assert_eq!(d.numero_dossier.as_deref(), Some("T-133/08"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(
                &[
                    "T-133/08".to_string(),
                    "T-134/08".to_string(),
                    "T-177/08".to_string(),
                    "T-242/09".to_string(),
                ][..]
            )
        );
    }

    #[test]
    fn cjue_celex_to_case_number_variants() {
        assert_eq!(celex_to_case_number("62020CJ0560"), "C-560/20");
        assert_eq!(celex_to_case_number("62023CO0614"), "C-614/23");
        assert_eq!(celex_to_case_number("62023TJ0188"), "T-188/23");
        assert_eq!(celex_to_case_number("62016CJ0550"), "C-550/16");
        assert_eq!(celex_to_case_number("NOTACELEX"), "NOTACELEX");
        assert_eq!(celex_to_case_number("32020R0560"), "32020R0560");
    }

    #[test]
    fn cjue_ecli_none_when_absent_and_errors_on_empty() {
        let predicates = json!({ "case-law_ecli": "", "work_date_document": "2024-01-30" });
        let d =
            parse_cjue("texte de l'arrêt", &predicates, "62020CJ0560", "fr").expect("parse CJUE");
        assert_eq!(d.ecli, None);

        assert!(parse_cjue("texte", &predicates, "", "fr").is_err());
        assert!(parse_cjue("  ", &predicates, "62020CJ0560", "fr").is_err());
    }

    #[test]
    fn vocab_cedh_cjue_labels_and_types() {
        assert_eq!(
            vocab::jurisdiction_type(Some("cedh")).as_deref(),
            Some("CEDH")
        );
        assert_eq!(
            vocab::jurisdiction_type(Some("cjue")).as_deref(),
            Some("CJUE")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("cedh")).as_deref(),
            Some("Cour européenne des droits de l'homme")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("cjue")).as_deref(),
            Some("Cour de justice de l'Union européenne")
        );
    }

    #[test]
    fn vocab_constit_tc_labels_and_types() {
        assert_eq!(
            vocab::jurisdiction_label(Some("constit")).as_deref(),
            Some("Conseil constitutionnel")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("tc")).as_deref(),
            Some("Tribunal des conflits")
        );
        assert_eq!(
            vocab::jurisdiction_type(Some("constit")).as_deref(),
            Some("CONSTIT")
        );
        assert_eq!(vocab::jurisdiction_type(Some("tc")).as_deref(), Some("TC"));
    }

    // ── Nom d'usage porté par la source ──────────────────────────────────────

    #[test]
    fn cedh_case_name_drops_the_label_and_the_publication_mention() {
        assert_eq!(
            cedh_case_name("AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE").as_deref(),
            Some("TAMAKULOVA ET AUTRES c. UKRAINE")
        );
        assert_eq!(
            cedh_case_name("CASE OF LUKOŠIN v. LITHUANIA").as_deref(),
            Some("LUKOŠIN v. LITHUANIA")
        );
        assert_eq!(
            cedh_case_name("AFFAIRES GADZHIYEV ET GOSTEV c. RUSSIE").as_deref(),
            Some("GADZHIYEV ET GOSTEV c. RUSSIE")
        );
        assert_eq!(
            cedh_case_name("ARRET LUTZ c. FRANCE").as_deref(),
            Some("LUTZ c. FRANCE")
        );
        assert_eq!(
            cedh_case_name("AFFAIRE DEDOVSKI ET AUTRES c. RUSSIE [Extraits]").as_deref(),
            Some("DEDOVSKI ET AUTRES c. RUSSIE")
        );
        // Le numéro d'ordre distingue deux affaires des mêmes parties : il reste.
        assert_eq!(
            cedh_case_name("CASE OF MASLÁK v. SLOVAKIA (No. 3)").as_deref(),
            Some("MASLÁK v. SLOVAKIA (No. 3)")
        );
    }

    #[test]
    fn cedh_case_name_refuses_what_names_nobody() {
        for docname in [
            "AFFAIRE K c. FRANCE",
            "AFFAIRE N.T.P. ET AUTRES c. FRANCE",
            "CASE OF S.S. AND J.H. v. NORWAY",
            "AFFAIRE T.Ç. ET H.Ç.c. TURQUIE",
            "AFFAIRE « RELATIVE À CERTAINS ASPECTS DU RÉGIME LINGUISTIQUE DE \
             L'ENSEIGNEMENT EN BELGIQUE » (FOND)",
        ] {
            assert_eq!(cedh_case_name(docname), None, "docname {docname:?}");
        }
        // Un nom court reste un nom.
        assert_eq!(
            cedh_case_name("AFFAIRE GREENS ET M.T. c. ROYAUME-UNI [Extraits]").as_deref(),
            Some("GREENS ET M.T. c. ROYAUME-UNI")
        );
        assert_eq!(
            cedh_case_name("AFFAIRE APBP c. FRANCE").as_deref(),
            Some("APBP c. FRANCE")
        );
    }

    /// Tête de texte EUR-Lex telle qu'elle est stockée : bandeau de quatre
    /// lignes, puis la ligne de titre.
    fn eurlex(celex: &str, title: &str) -> String {
        format!("EUR-Lex - {celex} - FR\nAvis juridique important\n|\n{celex}\n{title}\nRecueil de jurisprudence 1992 page I-06613\nSommaire\n")
    }

    #[test]
    fn cjue_case_name_reads_the_parties_field_of_the_title_line() {
        let arret = eurlex(
            "61991J0144",
            "Arrêt de la Cour (sixième chambre) du 16 décembre 1992. - Gilbert Demoor en \
             Zonen NV et autres contre &Eacute;tat belge. - Demandes de décision \
             préjudicielle: Rechtbank van eerste aanleg Brussel - Belgique. - Taxes \
             parafiscales. - Affaires jointes C-144/91 et C-145/91.",
        );
        assert_eq!(
            cjue_case_name(&arret).as_deref(),
            Some("Gilbert Demoor en Zonen NV et autres contre État belge")
        );

        let conclusions = eurlex(
            "61990C0087",
            "Conclusions de l'avocat général Darmon présentées le 29 mai 1991. - A. Verholen \
             et autres contre Sociale Verzekeringsbank Amsterdam. - Demandes de décision \
             préjudicielle: Raad van Beroep 's-Hertogenbosch - Pays-Bas. - Egalité entre \
             hommes et femmes. - Affaires jointes C-87/90, C-88/90 et C-89/90.",
        );
        assert_eq!(
            cjue_case_name(&conclusions).as_deref(),
            Some("A. Verholen et autres contre Sociale Verzekeringsbank Amsterdam")
        );

        let penal = eurlex(
            "62002J0012",
            "Arrêt de la Cour (première chambre) du 2 octobre 2003. - Procédure pénale contre \
             Marco Grilli. - Libre circulation. - Affaire C-12/02.",
        );
        assert_eq!(
            cjue_case_name(&penal).as_deref(),
            Some("Procédure pénale contre Marco Grilli")
        );
    }

    #[test]
    fn cjue_case_name_refuses_outside_the_title_line() {
        // Texte moderne : ni bandeau ni ligne de titre, les parties ne se lisent
        // que dans le corps.
        let moderne = "DOCUMENT DE TRAVAIL\nARRÊT DU TRIBUNAL (huitième chambre)\n\
                       27 septembre 2023 (*)\n« Aides d'État. Régime d'aides. »\n\
                       Dans l'affaire T-252/15,\nFerrovial, SA, établie à Madrid (Espagne),\n";
        assert_eq!(cjue_case_name(moderne), None);

        // Titre sans champ de parties : l'objet occupe la place.
        let sans_parties = eurlex(
            "61979O0105",
            "Ordonnance de la Cour du 27 juin 1979. - Demande de décision préjudicielle du \
             Juge chargé du service du tribunal d'instance. - Affaire 105/79.",
        );
        assert_eq!(cjue_case_name(&sans_parties), None);

        // Fonctionnaire anonymisé.
        let anonyme = eurlex(
            "61998A0084",
            "Judgment of the Court of First Instance (First Chamber) of 16 June 2000. - C v \
             Council of the European Union. - Officials. - Case T-84/98.",
        );
        assert_eq!(cjue_case_name(&anonyme), None);
    }

    #[test]
    fn cjue_title_case_name_reads_the_second_field() {
        assert_eq!(
            cjue_title_case_name(
                "Ordonnance du Tribunal (deuxième chambre) du 6 août 2024.#Sparkasse \
                 Oberösterreich Bank AG contre Conseil de résolution unique.#Union bancaire.#\
                 Affaire T-408/23."
            )
            .as_deref(),
            Some("Sparkasse Oberösterreich Bank AG contre Conseil de résolution unique")
        );
        assert_eq!(
            cjue_title_case_name(
                "Arrêt de la Cour.#XU e.a. contre Credit Europe Bank N.V.#Affaire C-1/19."
            )
            .as_deref(),
            Some("XU e.a. contre Credit Europe Bank N.V.")
        );
        assert_eq!(
            cjue_title_case_name(
                "Conclusions de l'avocat général Mme J. Kokott, présentées le 4 juin 2020."
            ),
            None
        );
        assert_eq!(
            cjue_title_case_name("Arrêt de la Cour.#NK.#Affaire C-231/18."),
            None
        );
    }

    // ── Round-trip `from_source_fields` ⟷ parse direct ───────────────────────

    #[test]
    fn cedh_round_trips_via_source_fields() {
        let columns = json!({
            "itemid": "001-250438",
            "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE",
            "appno": "20890/16;19185/25",
            "ecli": "ECLI:CE:ECHR:2026:0611JUD002089016",
            "judgementdate": "2026-06-11T00:00:00",
            "doctype": "HFJUD",
        });
        let body = "Vu : la procédure suivie devant la Cour. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour décide.";
        let orig = parse_cedh(body, &columns, "001-250438").expect("parse CEDH");
        let sf = build_source_fields(&columns, &orig.sections);
        let rebuilt =
            Decision::from_source_fields(&orig.texte_integral_clean, &sf, &orig.source_uid);
        assert_eq!(orig, rebuilt);
    }

    #[test]
    fn cjue_round_trips_via_source_fields() {
        let predicates = json!({
            "case-law_ecli": "ECLI:EU:C:2024:96",
            "work_date_document": "2024-01-30",
            "resource_legal_type": "CJ",
        });
        let body = "ARRÊT DE LA COUR (grande chambre) 30 janvier 2024. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour dit pour droit.";
        let orig = parse_cjue(body, &predicates, "62020CJ0560", "fr").expect("parse CJUE");
        let sf = build_source_fields(&predicates, &orig.sections);
        let rebuilt =
            Decision::from_source_fields(&orig.texte_integral_clean, &sf, &orig.source_uid);
        assert_eq!(orig, rebuilt);
    }

    #[test]
    fn an_eu_act_drops_the_eurlex_notice_and_the_journal_page_head() {
        let notice = "EUR-Lex - 31983R0642 - FR\nAvis juridique important\n|\n31983R0642\nRèglement (CEE) no 642/83 du Conseil du 15 mars 1983\nJournal officiel n° L 076 du 22/03/1983 p. 0001 - 0001\nédition spéciale espagnole: chapitre 03 tome 29 p. 0156\n*****\nRÈGLEMENT (CEE) No 642/83 DU CONSEIL\ndu 15 mars 1983";
        assert!(eurlex_act_text(notice).starts_with("RÈGLEMENT (CEE) No 642/83 DU CONSEIL\n"));
        let xhtml = "L_202500078FR.000101.fmx.xml\nJournal officiel\nde l'Union européenne\nFR\nSérie L\n2025/78\n15.1.2025\nRÈGLEMENT D’EXÉCUTION (UE) 2025/78 DE LA COMMISSION";
        assert!(eurlex_act_text(xhtml).starts_with("RÈGLEMENT D’EXÉCUTION"));
        let pdf = "N0 L 168 / 16\nJournal officiel des Communautés européennes\n15 . 6 . 82\nI\n(Actes dont la publication est une condition de leur applicabilité)\nREGLEMENT (CEE) N° 1510/82 DE LA COMMISSION";
        assert!(eurlex_act_text(pdf).starts_with("REGLEMENT (CEE) N° 1510/82"));
        let directive = "Journal officiel des Communautés européennes 11 . 4. 69\nDIRECTIVE DU CONSEIL\ndu 28 mars 1969";
        assert!(eurlex_act_text(directive).starts_with("DIRECTIVE DU CONSEIL"));
        let untitled =
            "Journal officiel des Communautés européennes\nLE CONSEIL DES COMMUNAUTÉS EUROPÉENNES,";
        assert_eq!(eurlex_act_text(untitled), untitled);
    }
}
