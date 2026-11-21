//! A JuriCAF decision page (AHJUCAF's francophone case law) into a [`Decision`] and its
//! `source_fields`: the facts from the page's Synthèse block, the court segment and number from the
//! URN:LEX it carries (`urn:lex;sn;cour.supreme;arret;2024-11-14;57`), the text from its article or,
//! when the article is empty, from an archived copy of what the page links.

use serde_json::{json, Value};

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

/// A decision text a page lacks, read from an archived copy of what it links.
#[derive(Debug)]
pub struct Recovered {
    pub text: String,
    /// The copy it was read from.
    pub url: String,
}

/// Below this many non-blank characters a text is a stub, not a decision.
const DECISION_FLOOR: usize = 200;

fn holds_a_decision(text: &str) -> bool {
    text.chars().filter(|c| !c.is_whitespace()).count() >= DECISION_FLOOR
}

/// Résultat du parse d'une page JuriCAF.
#[derive(Debug)]
pub struct JuricafParsed {
    pub decision: Decision,
    pub source_fields: Value,
    /// Code juridiction canonique `{iso}_{cour}` dérivé de l'URN:LEX
    /// (`sn_cour_supreme`) — clé du référentiel ouvert `jurisdiction`.
    pub jurisdiction_code: String,
    /// Libellé affichable (« Cour suprême (Sénégal) »).
    pub jurisdiction_label: String,
}

/// Décode les octets d'une page JuriCAF : UTF-8 nominal, chaque octet
/// invalide isolé passé en Windows-1252 (la plage 0x80-0x9F porte les
/// apostrophes typographiques). Les pages du fond sont MIXTES — Liban :
/// corps UTF-8 avec quelques octets 1252 crus — donc le repli est par
/// octet, jamais par page entière (une page basculée en bloc mojibake
/// tout son UTF-8 valide).
fn decode_page(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&rest[..valid]).expect("préfixe validé"));
                out.push(cp1252_char(rest[valid]));
                rest = &rest[valid + 1..];
            }
        }
    }
}

/// Caractère Windows-1252 d'un octet ≥ 0x80 (plage 0x80-0x9F remappée, le
/// reste coïncide avec Latin-1).
fn cp1252_char(b: u8) -> char {
    match b {
        0x80 => '€',
        0x82 => '‚',
        0x84 => '„',
        0x85 => '…',
        0x8a => 'Š',
        0x8c => 'Œ',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x9a => 'š',
        0x9c => 'œ',
        0xa0 => ' ',
        b => b as char,
    }
}

/// Déplie les entités HTML courantes du fond (pas de parseur complet : les
/// pages JuriCAF n'émettent que le répertoire XHTML classique).
fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&eacute;", "é")
        .replace("&egrave;", "è")
        .replace("&agrave;", "à")
        .replace("&ccedil;", "ç")
}

/// Texte brut d'un fragment HTML : balises → sauts de ligne, entités
/// dépliées, lignes vides compactées.
fn strip_tags(html: &str) -> String {
    // Avance par octets ; on n'indexe la chaîne que sur `<`/`>` (ASCII, donc
    // toujours frontière de char en UTF-8) — jamais via une copie
    // `to_lowercase()` dont les longueurs d'octets divergent.
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => {
                if bytes.len() - i >= 7 && bytes[i..i + 7].eq_ignore_ascii_case(b"<script") {
                    in_script = true;
                }
                if bytes.len() - i >= 9 && bytes[i..i + 9].eq_ignore_ascii_case(b"</script>") {
                    in_script = false;
                }
                in_tag = true;
                out.push('\n');
                i += 1;
            }
            b'>' => {
                in_tag = false;
                i += 1;
            }
            _ if !in_tag && !in_script => {
                // Frontière garantie : on n'arrive ici que depuis un `>`
                // (ASCII) ou après un `len_utf8()` complet.
                let c = html[i..].chars().next().expect("frontière UTF-8");
                out.push(c);
                i += c.len_utf8();
            }
            _ => i += 1,
        }
    }
    let text = unescape(&out);
    let mut lines: Vec<&str> = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if !t.is_empty() {
            lines.push(t);
        }
    }
    lines.join("\n")
}

/// The foreign court types, by the leading words of the URN:LEX court segment.
const COURT_FAMILIES: [(&[&str], &str); 23] = [
    (&["cour", "supreme"], "COUR_SUPREME"),
    (&["cour", "populaire", "supreme"], "COUR_SUPREME"),
    (&["cour", "populaire", "centrale"], "COUR_SUPREME"),
    (&["tribunal", "superieur", "justice"], "COUR_SUPREME"),
    (&["cour", "cassation"], "COUR_CASSATION"),
    (&["haute", "cour", "cassation"], "COUR_CASSATION"),
    (&["conseil", "constitutionnel"], "COUR_CONSTITUTIONNELLE"),
    (&["cour", "constitutionnelle"], "COUR_CONSTITUTIONNELLE"),
    (
        &["haute", "cour", "constitutionnelle"],
        "COUR_CONSTITUTIONNELLE",
    ),
    (
        &["haute", "cour", "constitutionelle"],
        "COUR_CONSTITUTIONNELLE",
    ),
    (&["cour", "appel"], "COUR_APPEL"),
    (&["cour", "administrative"], "COUR_ADMINISTRATIVE"),
    (&["tribunal", "administratif"], "TRIBUNAL_ADMINISTRATIF"),
    (
        &["tribunal", "premiere", "instance"],
        "TRIBUNAL_PREMIERE_INSTANCE",
    ),
    (
        &["tribunal", "grande", "instance"],
        "TRIBUNAL_PREMIERE_INSTANCE",
    ),
    (&["high", "court"], "TRIBUNAL_PREMIERE_INSTANCE"),
    (&["tribunal", "commerce"], "TRIBUNAL_COMMERCE"),
    (&["cour", "criminelle"], "COUR_CRIMINELLE"),
    (&["cour", "justice"], "COUR_JUSTICE"),
    (&["cour", "commune", "justice", "arbitrage"], "CCJA"),
    (
        &["cour", "africaine", "droits", "homme"],
        "COUR_DROITS_HOMME",
    ),
    (
        &["cour", "africaine", "droits", "homme", "et", "peuples"],
        "COUR_DROITS_HOMME",
    ),
    (
        &["cour", "interamericaine", "droits", "homme"],
        "COUR_DROITS_HOMME",
    ),
];

/// The families whose head ends on the court's name within its type: Niger's Cour d'État, its
/// supreme court since 2025, keeps a code apart from the Cour suprême.
const NAMED_FAMILIES: [(&[&str], &str, &[&str]); 1] =
    [(&["cour", "etat"], "COUR_SUPREME", &["etat"])];

/// The name of a court whose pages spell it more than one way: Niger's Cour d'État is « Cour
/// d'etat » on its pages of 2013.
const COURT_NAMES: [(&str, &str); 1] = [("ne_cour_supreme_etat", "Cour d'État")];

/// A court JuriCAF files under the name it took later, its decisions before that day placed under
/// the court they were given by: (code after, first day after, code before, type before, name
/// before). Morocco's law 58-11 renamed its Cour suprême on 26/10/2011.
const RENAMED: [(&str, &str, &str, &str, &str); 1] = [(
    "ma_cour_cassation",
    "2011-10-26",
    "ma_cour_supreme",
    "COUR_SUPREME",
    "Cour suprême",
)];

/// The words a court's name links with, which neither its code nor a country's carries.
const LINKING: [&str; 7] = ["de", "du", "des", "la", "le", "les", "of"];

/// The court segment in lower case without its elisions and linking words, as most URNs already
/// write it.
fn plain_court(court: &str) -> String {
    let court = court.split_once(',').map_or(court, |(head, _)| head);
    court
        .to_lowercase()
        .replace(' ', ".")
        .split('.')
        .map(|w| {
            ["d'", "l'", "d\u{2019}", "l\u{2019}"]
                .iter()
                .find_map(|elision| w.strip_prefix(elision))
                .unwrap_or(w)
        })
        .filter(|w| !w.is_empty() && !LINKING.contains(w))
        .collect::<Vec<_>>()
        .join(".")
}

/// A country's name as the words a court segment would write it in.
fn country_words(country: &str) -> Vec<String> {
    lj_fold::fold(country)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1 && !LINKING.contains(w))
        .map(str::to_string)
        .collect()
}

/// The type and code of a court, and whether its code names more than its type. A parenthesised
/// word repeats the organisation, and a name ending on its `country` is that country's court.
fn court_identity(
    place: &str,
    court: &str,
    country: &[&str],
) -> Option<(&'static str, String, bool)> {
    let words: Vec<&str> = court.split('.').filter(|w| !w.starts_with('(')).collect();
    let (head, kind, kept) = NAMED_FAMILIES
        .iter()
        .find(|(head, _, _)| words.starts_with(head))
        .copied()
        .or_else(|| {
            COURT_FAMILIES
                .iter()
                .filter(|(head, _)| words.starts_with(head))
                .max_by_key(|(head, _)| head.len())
                .map(|&(head, kind)| (head, kind, &[][..]))
        })?;
    let mut name = &words[head.len()..];
    if !country.is_empty() && name.ends_with(country) {
        name = &name[..name.len() - country.len()];
    }
    let name: Vec<&str> = kept.iter().chain(name).copied().collect();
    let code = lj_vocab::courts::foreign_court_code(place, kind, &name);
    Some((kind, code, !name.is_empty()))
}

/// What a page's « Synthèse » block states of its decision.
struct Facts {
    country: String,
    jurisdiction: String,
    tribunal: Option<String>,
    date: String,
    /// « Numéro d'arrêt », which a page without a URN:LEX is numbered by.
    number: Option<String>,
}

impl Facts {
    /// Each label of the block stands on its own line once the tags are stripped, its value on
    /// the next.
    fn read(html: &str) -> std::result::Result<Self, String> {
        let start = html.find("Synthèse</h5>").ok_or("no Synthèse block")? + "Synthèse</h5>".len();
        let end = html[start..].find("<hr").map_or(html.len(), |i| start + i);
        let block = strip_tags(&html[start..end]);
        let lines: Vec<&str> = block.lines().collect();
        let field = |label: &str| {
            lines
                .iter()
                .position(|l| l.strip_suffix(':').is_some_and(|k| k.trim_end() == label))
                .and_then(|i| lines.get(i + 1))
                .map(|v| v.to_string())
        };
        let date = field("Date de la décision").ok_or("no decision date")?;
        let number = lines
            .iter()
            .find_map(|l| l.trim().strip_prefix("Numéro d'arrêt"))
            .map(|v| v.trim_start_matches([' ', ':']).trim().to_string())
            .filter(|v| !v.is_empty());
        Ok(Self {
            country: field("Pays").ok_or("no country")?,
            jurisdiction: field("Juridiction").ok_or("no court")?,
            tribunal: field("Tribunal"),
            date: iso_date(&date).ok_or_else(|| format!("decision date {date:?}"))?,
            number,
        })
    }
}

/// ISO date of a `dd/mm/yyyy`.
fn iso_date(s: &str) -> Option<String> {
    let [d, m, y] = s.split('/').collect::<Vec<_>>()[..] else {
        return None;
    };
    if y.len() != 4 || !y.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (d, m): (u32, u32) = (d.parse().ok()?, m.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(format!("{y}-{m:02}-{d:02}"))
}

/// The code of a country or an organisation in the court codes, by the name a page gives it.
fn country_code(name: &str) -> Option<&'static str> {
    Some(match name {
        "Andorre" => "ad",
        "Belgique" => "be",
        "Bénin" => "bj",
        "Bulgarie" => "bg",
        "Burkina Faso" => "bf",
        "Burundi" => "bi",
        "CADHP" => "unionafricaine",
        "Cambodge" => "kh",
        "Cameroun" => "cm",
        "Canada" => "ca",
        "CEDEAO" => "cedeao",
        "CEMAC" => "cemac",
        "Comores" => "km",
        "Congo" => "cg",
        "Congo démocratique" => "cd",
        "Côte d'Ivoire" => "ci",
        "Egypte" => "eg",
        "Gabon" => "ga",
        "Guinée" => "gn",
        "Haïti" => "ht",
        "Hongrie" => "hu",
        "Liban" => "lb",
        "Luxembourg" => "lu",
        "Madagascar" => "mg",
        "Mali" => "ml",
        "Maroc" => "ma",
        "Maurice" => "mu",
        "Mauritanie" => "mr",
        "Monaco" => "mc",
        "Niger" => "ne",
        "OHADA" => "ohada",
        "République centrafricaine" => "cf",
        "Rwanda" => "rw",
        "Sao Tomé et Principe" => "st",
        "Sénégal" => "sn",
        "Suisse" => "ch",
        "Tchad" => "td",
        "Togo" => "tg",
        "Tunisie" => "tn",
        "UEMOA" => "uemoa",
        _ => return None,
    })
}

/// The words of a court's name that say what it is, folded: they stay in lower case, and every
/// other word names a place.
const COURT_WORDS: [&str; 42] = [
    "administratif",
    "administrative",
    "africaine",
    "appel",
    "arbitrage",
    "cassation",
    "centrale",
    "chambre",
    "classe",
    "commerce",
    "commune",
    "conseil",
    "constitutionelle",
    "constitutionnel",
    "constitutionnelle",
    "cour",
    "criminelle",
    "de",
    "des",
    "droits",
    "du",
    "economique",
    "en",
    "et",
    "grande",
    "haute",
    "homme",
    "hors",
    "instance",
    "interamericaine",
    "justice",
    "la",
    "le",
    "les",
    "monetaire",
    "of",
    "peuples",
    "populaire",
    "premiere",
    "superieur",
    "supreme",
    "tribunal",
];

/// A court's name as the source writes it, the places it names in lower case, given their capital.
fn capitalise_places(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, piece) in name.split_inclusive([' ', '-']).enumerate() {
        let word = piece.trim_end_matches([' ', '-']);
        let elided = word
            .find(['\'', '\u{2019}'])
            .filter(|&at| at == 1)
            .map_or(0, |at| {
                at + word[at..].chars().next().map_or(0, char::len_utf8)
            });
        let (elision, stem) = word.split_at(elided);
        out.push_str(elision);
        let mut chars = stem.chars();
        match chars.next() {
            Some(first) if i == 0 || !COURT_WORDS.contains(&lj_fold::fold(stem).as_str()) => {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
            _ => out.push_str(stem),
        }
        out.push_str(&piece[word.len()..]);
    }
    out
}

/// The decision a page states: its court coded from the URN's court segment, in the country the
/// page names, and named by the page.
fn decision_from_facts(
    slug: &str,
    court: &str,
    num: &str,
    facts: &Facts,
    text: String,
) -> std::result::Result<Decision, String> {
    let place = country_code(&facts.country)
        .ok_or_else(|| format!("country {:?} has no code", facts.country))?;
    let court = plain_court(court);
    let country = country_words(&facts.country);
    let country: Vec<&str> = country.iter().map(String::as_str).collect();
    let (kind, code, named) = court_identity(place, &court, &country)
        .ok_or_else(|| format!("court of no foreign type: {court}"))?;
    let renamed = RENAMED
        .iter()
        .find(|(after, since, ..)| *after == code && facts.date.as_str() < *since);
    let (kind, code, fixed) = match renamed {
        Some(&(_, _, before, kind, name)) => (kind, before.to_string(), Some(name)),
        None => {
            let fixed = COURT_NAMES
                .iter()
                .find(|(c, _)| *c == code)
                .map(|&(_, n)| n);
            (kind, code, fixed)
        }
    };
    let name = match (fixed, &facts.tribunal) {
        (Some(name), _) => name.to_string(),
        (None, Some(tribunal)) if named => capitalise_places(tribunal),
        _ => capitalise_places(&facts.jurisdiction),
    };
    let label = if lj_fold::fold(&name).contains(&lj_fold::fold(&facts.country)) {
        name
    } else {
        format!("{name} ({})", facts.country)
    };
    let numero = num.replace('.', "-");
    Ok(Decision {
        source_uid: format!("juricaf/{slug}"),
        member_name: slug.to_string(),
        ecli: None,
        jurisdiction_source_code: Some(code),
        chamber: None,
        nac: None,
        jurisdiction_name: Some(label),
        jurisdiction_type: Some(kind.to_string()),
        jurisdiction_location: None,
        numero_dossier: Some(numero.clone()),
        numero_dossiers: Some(vec![numero]),
        numero_role: None,
        date_lecture: Some(facts.date.clone()),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
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
    })
}

/// JuriCAF branch of [`Decision::from_source_fields`]: the decision `parse_juricaf` built.
pub(crate) fn from_source_fields_juricaf(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let field = |k: &str| source_fields.get(k).and_then(Value::as_str);
    let stored = |k: &str| {
        field(k)
            .unwrap_or_else(|| panic!("source_fields juricaf without {k}: {source_uid}"))
            .to_string()
    };
    let (court, num) = match field("urn_lex") {
        Some(urn) => {
            let [_, court, _, _, num] = urn.split(';').collect::<Vec<_>>()[..] else {
                panic!("source_fields juricaf: malformed URN:LEX {urn} ({source_uid})");
            };
            (court.to_string(), num.to_string())
        }
        None => (stored("court_segment"), stored("number")),
    };
    let facts = Facts {
        country: stored("country"),
        jurisdiction: stored("jurisdiction"),
        tribunal: field("tribunal").map(str::to_string),
        date: stored("date"),
        number: None,
    };
    let slug = source_uid.strip_prefix("juricaf/").unwrap_or(source_uid);
    decision_from_facts(slug, &court, &num, &facts, full_text.to_string())
        .unwrap_or_else(|e| panic!("stored juricaf decision: {e} ({source_uid})"))
}

/// The decision text a page's article holds, if it holds one.
pub fn juricaf_article(raw: &[u8]) -> Option<String> {
    article(&decode_page(raw)).filter(|text| holds_a_decision(text))
}

/// The decision text an archived page holds: the full-text field of a jurisprudence.ma page, or
/// the article of a JuriCAF page.
pub fn archived_text(raw: &[u8]) -> Option<String> {
    let html = decode_page(raw);
    full_text_field(&html)
        .or_else(|| article(&html))
        .filter(|text| holds_a_decision(text))
}

/// The decision an OHADATA file holds after its reference, the editor's additions before it left out.
pub fn ohadata_decision(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let reference = (0..lines.len()).find(|&i| {
        let head = lines[i].trim_start().trim_start_matches('(');
        ["COUR ", "TRIBUNAL ", "CONSEIL "]
            .iter()
            .any(|w| head.starts_with(w))
            && !head.contains(" - ")
            && lines[i..]
                .iter()
                .take(3)
                .take_while(|l| !l.trim().is_empty())
                .any(|l| l.contains("n°"))
    })?;
    let body = reference
        + lines[reference..]
            .iter()
            .position(|l| l.trim().is_empty())?;
    Some(lines[body..].join("\n").trim().to_string()).filter(|t| holds_a_decision(t))
}

/// jurisprudence.ma's full text: its Arabic field before 2021, else its French one, then the block
/// under « Texte intégral ».
fn full_text_field(html: &str) -> Option<String> {
    let block = match html
        .find("field-name-field-texte-integral-ar")
        .or_else(|| html.find("field-name-field-texte-integral-fr"))
    {
        Some(field) => field + html[field..].find("<div class=\"field-item")?,
        None => {
            let heading = html.find(">Texte intégral</h4>")?;
            heading + html[heading..].find("<div class=\"is-edito")?
        }
    };
    div_inner(html, block).map(strip_tags)
}

/// The inner HTML of the `<div` opening at `start`, nested divs included.
fn div_inner(html: &str, start: usize) -> Option<&str> {
    let inner = start + html[start..].find('>')? + 1;
    let (mut depth, mut at) = (1, inner);
    while depth > 0 {
        let close = at + html[at..].find("</div>")?;
        match html[at..close].find("<div") {
            Some(open) => {
                depth += 1;
                at += open + 4;
            }
            None => {
                depth -= 1;
                at = close + 6;
            }
        }
    }
    Some(&html[inner..at - 6])
}

/// `ara` when Arabic letters outnumber Latin ones, `fra` otherwise.
pub(crate) fn script_language(text: &str) -> &'static str {
    let arabic = text
        .chars()
        .filter(|c| ('\u{0600}'..='\u{06ff}').contains(c))
        .count();
    let latin = text.chars().filter(char::is_ascii_alphabetic).count();
    if arabic > latin {
        "ara"
    } else {
        "fra"
    }
}

/// Parses a JuriCAF decision page. `slug` is the URL segment after `/arret/`
/// (`SENEGAL-COURSUPREME-20241114-57`), the pivot of the `source_uid`.
pub fn parse_juricaf(
    slug: &str,
    raw: &[u8],
    recovered: Option<Recovered>,
) -> Result<JuricafParsed> {
    let html = decode_page(raw);

    let urn = html
        .split("urn:lex;")
        .nth(1)
        .and_then(|rest| rest.split(['"', '<', '\n', '\r']).next())
        .map(|urn| urn.trim().trim_end_matches(&[';', '.'][..]).to_string());
    let facts = Facts::read(&html).map_err(|e| CoreError::Xml(format!("juricaf {slug}: {e}")))?;
    // A page without a URN:LEX names its court and number in its synthesis alone.
    let (court, num) = match &urn {
        Some(urn) => {
            let segs: Vec<&str> = urn.split(';').collect();
            let [_, court, _nature, _, num] = segs.as_slice() else {
                return Err(CoreError::Xml(format!(
                    "juricaf {slug}: URN:LEX à {} segments (5 attendus) : {urn}",
                    segs.len()
                )));
            };
            (court.to_string(), num.to_string())
        }
        None => {
            let num = facts.number.clone().ok_or_else(|| {
                CoreError::Xml(format!("juricaf {slug}: neither a URN:LEX nor a number"))
            })?;
            let court = lj_fold::fold(&facts.jurisdiction)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(".");
            (court, num)
        }
    };

    let title = html
        .split("<title>")
        .nth(1)
        .and_then(|r| r.split("</title>").next())
        .map(|t| unescape(t.trim()).to_string())
        .unwrap_or_else(|| slug.to_string());

    let (text, recovered_from) = match article(&html).filter(|t| holds_a_decision(t)) {
        Some(text) => (text, None),
        None => match recovered.filter(|r| holds_a_decision(&r.text)) {
            Some(r) => (r.text, Some(r.url)),
            None => return Err(CoreError::Xml(format!("juricaf {slug}: no decision text"))),
        },
    };

    let lang = match &recovered_from {
        Some(_) => script_language(&text),
        None => {
            let language = meta_content(&html, "dc.language").unwrap_or_default();
            lj_vocab::languages::eu_of_iso_639_1(&language.to_ascii_lowercase()).ok_or_else(
                || CoreError::Xml(format!("juricaf {slug}: page language {language:?}")),
            )?
        }
    };

    let mut source_fields = json!({
        "urn_lex": urn,
        "juricaf_slug": slug,
        "title": title,
        "lang": lang,
        "source_url": format!("https://juricaf.org/arret/{slug}"),
        "country": facts.country,
        "jurisdiction": facts.jurisdiction,
        "tribunal": facts.tribunal,
        "date": facts.date,
    });
    if let Some(url) = recovered_from {
        source_fields["text_url"] = json!(url);
    }
    if urn.is_none() {
        source_fields["court_segment"] = json!(court);
        source_fields["number"] = json!(num);
    }

    let decision = decision_from_facts(slug, &court, &num, &facts, text)
        .map_err(|e| CoreError::Xml(format!("juricaf {slug}: {e}")))?;
    let jurisdiction_label = decision
        .jurisdiction_name
        .clone()
        .expect("decision_from_facts names the court");

    Ok(JuricafParsed {
        jurisdiction_code: decision
            .jurisdiction_source_code
            .clone()
            .expect("decision_from_facts codes the court"),
        decision,
        source_fields,
        jurisdiction_label,
    })
}

/// The decision's text: the page's article, without the summary it opens with on a small screen
/// nor the side column.
fn article(html: &str) -> Option<String> {
    const OPEN: &str = "<article id=\"textArret\">";
    let start = html.find(OPEN)? + OPEN.len();
    let end = start + html[start..].find("</article>")?;
    Some(strip_tags(&html[start..end]))
}

fn meta_content<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let rest = html
        .split(&format!("<meta name=\"{name}\" content=\""))
        .nth(1)?;
    rest.split('"').next()
}

#[cfg(test)]
mod juricaf_tests {
    use super::*;

    #[test]
    fn decode_page_mixed_utf8_and_cp1252() {
        // Page libanaise réelle : corps UTF-8 valide + apostrophe 0x92 crue.
        let mut raw = "Assemblée plénière".as_bytes().to_vec();
        raw.extend_from_slice(b" l");
        raw.push(0x92);
        raw.extend_from_slice("arrêt".as_bytes());
        assert_eq!(decode_page(&raw), "Assemblée plénière l\u{2019}arrêt");
    }

    /// A page as JuriCAF lays it out: header, summary for small screens, article, side column.
    fn page(urn: &str, synthese: &str, article: &str) -> String {
        format!(
            "<html><head><title>T</title><meta content=\"urn:lex;{urn}\">\
             <meta name=\"dc.language\" content=\"FR\" /></head><body>\
             <div id=\"arret\"><small class=\"text-muted\"> 06/03/2009 | PAYS | N°1 </small>\
             <h1 id=\"titre\">Titre de la page</h1><span class='text-muted'>Texte (pseudonymisé) </span>\
             <div id=\"debutArret\">Aperçu</div><article id=\"textArret\"><p>{article}</p></article>\
             <h5>Synthèse</h5>{synthese}<br/><hr><h5>Analyses</h5>MOTS - CLES\
             <h5>Origine de la décision</h5></div></body></html>"
        )
    }

    fn synthese(country: &str, jurisdiction: &str, tribunal: Option<&str>, date: &str) -> String {
        let tribunal = tribunal
            .map(|t| format!("Tribunal : <a href=\"#\">{t}</a><br />"))
            .unwrap_or_default();
        format!(
            "<div itemprop=\"author\"><span itemprop=\"address\">Pays : <a href=\"#\">{country}</a>\
             </span><br>Juridiction : <span itemprop=\"name\"><a href=\"#\">{jurisdiction}</a>\
             </span></div>{tribunal}Numéro d'arrêt : 1<br/>Date de la décision : \
             <span itemprop=\"dateCreated\">{date}</span>"
        )
    }

    fn body() -> String {
        "Attendu que la cour statue en matière commerciale. ".repeat(10)
    }

    #[test]
    fn a_moroccan_cour_de_cassation_page_before_law_58_11_is_the_cour_supreme_s() {
        let parsed = |date: &str, day: &str| {
            let html = page(
                &format!("ma;cour.cassation;arret;{day};588"),
                &synthese("Maroc", "Cour de cassation", None, date),
                &body(),
            );
            parse_juricaf("MAROC-COURDECASSATION-X-588", html.as_bytes(), None).unwrap()
        };
        let before = parsed("25/10/2011", "2011-10-25");
        assert_eq!(before.jurisdiction_code, "ma_cour_supreme");
        assert_eq!(before.jurisdiction_label, "Cour suprême (Maroc)");
        assert_eq!(
            before.decision.jurisdiction_type.as_deref(),
            Some("COUR_SUPREME")
        );
        let rebuilt = from_source_fields_juricaf(
            &before.decision.texte_integral_clean,
            &before.source_fields,
            "juricaf/MAROC-COURDECASSATION-X-588",
        );
        assert_eq!(
            rebuilt.jurisdiction_source_code.as_deref(),
            Some("ma_cour_supreme")
        );
        let after = parsed("26/10/2011", "2011-10-26");
        assert_eq!(after.jurisdiction_code, "ma_cour_cassation");
        assert_eq!(after.jurisdiction_label, "Cour de cassation (Maroc)");
    }

    #[test]
    fn niger_s_cour_d_etat_keeps_its_name_however_its_page_spells_it() {
        let html = page(
            "ne;cour.etat;arret;2013-05-30;13.152.p",
            &synthese("Niger", "Cour d'etat", None, "30/05/2013"),
            &body(),
        );
        let p = parse_juricaf("NIGER-COURDETAT-20130530-13152P", html.as_bytes(), None).unwrap();
        assert_eq!(p.jurisdiction_code, "ne_cour_supreme_etat");
        assert_eq!(p.jurisdiction_label, "Cour d'État (Niger)");
    }

    #[test]
    fn a_page_without_a_urn_is_placed_and_numbered_by_its_synthesis() {
        let html = page(
            "x",
            &synthese(
                "Congo démocratique",
                "Cour de cassation",
                None,
                "26/06/2019",
            ),
            &body(),
        )
        .replace("<meta content=\"urn:lex;x\">", "")
        .replace(
            "Numéro d'arrêt : 1<br/>",
            "Numéro d'arrêt : RC107/RC3877<br/>",
        );
        let slug = "REPUBLIQUEDEMOCRATIQUEDUCONGO-COURDECASSATION-20190626-RC107RC3877";
        let p = parse_juricaf(slug, html.as_bytes(), None).unwrap();
        assert_eq!(p.jurisdiction_code, "cd_cour_cassation");
        assert_eq!(p.decision.date_lecture.as_deref(), Some("2019-06-26"));
        assert_eq!(p.decision.numero_dossier.as_deref(), Some("RC107/RC3877"));
        let rebuilt = from_source_fields_juricaf(
            &p.decision.texte_integral_clean,
            &p.source_fields,
            &format!("juricaf/{slug}"),
        );
        assert_eq!(
            rebuilt.jurisdiction_source_code.as_deref(),
            Some("cd_cour_cassation")
        );
        assert_eq!(rebuilt.numero_dossier.as_deref(), Some("RC107/RC3877"));
    }

    #[test]
    fn the_page_states_the_decision_its_urn_misdates() {
        let html = page(
            "bf;cour.appel.ouagadougou;arret;2909-03-06;010",
            &synthese(
                "Burkina Faso",
                "Cour d'appel",
                Some("Cour d'appel de ouagadougou"),
                "06/03/2009",
            ),
            &body(),
        );
        let p = parse_juricaf(
            "BURKINAFASO-COURDAPPELDEOUAGADOUGOU-20090306-010",
            html.as_bytes(),
            None,
        )
        .unwrap();
        assert_eq!(p.jurisdiction_code, "bf_cour_appel_ouagadougou");
        assert_eq!(p.decision.jurisdiction_type.as_deref(), Some("COUR_APPEL"));
        assert_eq!(p.decision.date_lecture.as_deref(), Some("2009-03-06"));
        assert_eq!(
            p.jurisdiction_label,
            "Cour d'appel de Ouagadougou (Burkina Faso)"
        );
        let text = &p.decision.texte_integral_clean;
        assert!(text.starts_with("Attendu que la cour"));
        for aside in [
            "PAYS",
            "Titre",
            "pseudonymisé",
            "Aperçu",
            "Synthèse",
            "MOTS",
        ] {
            assert!(!text.contains(aside), "{aside} in the text");
        }
        assert_eq!(p.source_fields["date"], "2009-03-06");
        assert_eq!(p.source_fields["lang"], "fra");
    }

    #[test]
    fn the_country_is_the_one_the_page_names() {
        let html = page(
            "sn;cour.supreme;arret;2019-06-21;260.ca",
            &synthese("Bénin", "Cour suprême", None, "21/06/2019"),
            &body(),
        );
        let p = parse_juricaf("BENIN-COURSUPREME-20190621-260CA", html.as_bytes(), None).unwrap();
        assert_eq!(p.jurisdiction_code, "bj_cour_supreme");
        assert_eq!(p.jurisdiction_label, "Cour suprême (Bénin)");
    }

    #[test]
    fn a_court_named_after_its_country_is_that_country_s_court() {
        let html = page(
            "cg;cour.supreme.du.congo;arret;2009-06-12;02",
            &synthese(
                "Congo",
                "Cour suprême",
                Some("Cour suprême du congo"),
                "12/06/2009",
            ),
            &body(),
        );
        let p = parse_juricaf(
            "CONGO-COURSUPREMEDUCONGO-20090612-02",
            html.as_bytes(),
            None,
        )
        .unwrap();
        assert_eq!(p.jurisdiction_code, "cg_cour_supreme");
        assert_eq!(p.jurisdiction_label, "Cour suprême (Congo)");
    }

    #[test]
    fn a_page_without_its_decision_text_takes_the_archived_one_else_is_refused() {
        let facts = synthese("Maroc", "Cour d'appel", None, "18/09/1969");
        let html = page("ma;cour.appel;arret;1969-09-18;1", &facts, "");
        let slug = "MAROC-COURDAPPEL-19690918-1";
        assert!(parse_juricaf(slug, html.as_bytes(), None).is_err());
        let stub = Recovered {
            text: "باسم جلالة الملك".to_string(),
            url: "https://web.archive.org/web/2016/http://www.jurisprudence.ma/decision/x".into(),
        };
        assert!(parse_juricaf(slug, html.as_bytes(), Some(stub)).is_err());

        let archived = format!(
            "<div class=\"section field field-name-field-texte-integral-ar field-type-text-long\">\
             <div class=\"field-items\"><div class=\"field-item odd\"><p>{}<br />{}</p>\
             </div></div></div><div class=\"panel-separator\"></div>",
            "باسم جلالة الملك ",
            "حكمت محكمة الاستئناف بالرباط ".repeat(10)
        );
        let text = archived_text(archived.as_bytes()).unwrap();
        assert!(text.starts_with("باسم جلالة الملك\nحكمت"));
        let relaunched = format!(
            "<h4 class=\"is-blue is-bold\">Texte intégral</h4>\n<div class=\"is-edito rtl\">\
             <div dir=\"rtl\">{}<br />{}</div></div><div class=\"is-edito rtl\">ملخص</div>",
            "باسم جلالة الملك",
            "حكمت محكمة الاستئناف بالرباط ".repeat(10)
        );
        let text = archived_text(relaunched.as_bytes()).unwrap();
        assert!(text.starts_with("باسم جلالة الملك\nحكمت") && !text.contains("ملخص"));
        let recovered = Recovered {
            text,
            url: "https://web.archive.org/web/2016/http://www.jurisprudence.ma/decision/x".into(),
        };
        let p = parse_juricaf(slug, html.as_bytes(), Some(recovered)).unwrap();
        assert!(p.decision.texte_integral_clean.starts_with("باسم"));
        assert_eq!(p.source_fields["lang"], "ara");
        assert_eq!(
            p.source_fields["text_url"],
            "https://web.archive.org/web/2016/http://www.jurisprudence.ma/decision/x"
        );
    }

    #[test]
    fn an_ohadata_file_keeps_the_decision_after_its_reference() {
        let decision = format!(
            "LA COUR,\n\nVu les pièces du dossier ;\n{}PAR CES MOTIFS\nConfirme le jugement.",
            "Attendu que l'appelant soutient que la vente est nulle ;\n".repeat(6)
        );
        let file = format!(
            "Ohadata J-00-001\n\nVENTE - PRIX - COUR D'APPEL - PAIEMENT (OUI)\n\n\
             Résumé de l'éditeur.\n\nARTICLE 1 AUDCG\nCOUR D'APPEL DE OUAGADOUGOU, Chambre \
             commerciale (BURKINA FASO), Arrêt\nn° 1 du 2 janvier 2009, A c/ B\n\n{decision}\n"
        );
        assert_eq!(ohadata_decision(&file).as_deref(), Some(decision.as_str()));
        let wrapped = file.replace("COUR D'APPEL DE", "(COUR D'APPEL DE");
        assert_eq!(
            ohadata_decision(&wrapped).as_deref(),
            Some(decision.as_str())
        );

        let abstract_only = "Ohadata J-00-002\n\nSAISIE - SOCIÉTÉ\nARTICLE 54 AUPSRVE\n\
             Cour d'appel de Brazzaville, Arrêt n° 1 du 4 février 2010. Revue, n° 1, p. 4.\n\
             Cette décision est disponible dans la Revue, n° 1, p. 4.\n";
        assert_eq!(ohadata_decision(abstract_only), None);
    }

    #[test]
    fn a_court_name_capitalises_its_places_alone() {
        for (source, name) in [
            ("Cour d'appel de ouagadougou", "Cour d'appel de Ouagadougou"),
            ("Cour d'appel de commerce", "Cour d'appel de commerce"),
            (
                "Tribunal de première instance de yaoundé ekounou",
                "Tribunal de première instance de Yaoundé Ekounou",
            ),
            (
                "Tribunal de grande instance du nyong-et-kéllé",
                "Tribunal de grande instance du Nyong-et-Kéllé",
            ),
            (
                "Tribunal de grande instance hors classe de dakar",
                "Tribunal de grande instance hors classe de Dakar",
            ),
            (
                "Cour de justice de la communauté économique et monétaire de l'afrique centrale",
                "Cour de justice de la Communauté économique et monétaire de l'Afrique centrale",
            ),
            (
                "Cour suprême de côte d'ivoire",
                "Cour suprême de Côte d'Ivoire",
            ),
            ("High court of mezam", "High Court of Mezam"),
        ] {
            assert_eq!(capitalise_places(source), name);
        }
    }

    #[test]
    fn a_court_segment_drops_a_chamber_after_a_comma_and_reads_a_space_as_a_dot() {
        assert_eq!(plain_court("Cour administrative"), "cour.administrative");
        assert_eq!(
            plain_court("cour.supreme,.chambre.administrative"),
            "cour.supreme"
        );
        assert_eq!(
            plain_court("cour.d\u{2019}appel..nouakchott"),
            "cour.appel.nouakchott"
        );
    }

    #[test]
    fn a_court_segment_opens_with_its_type_and_ends_with_its_name() {
        assert_eq!(
            court_identity("sn", "cour.appel.dakar", &[]),
            Some(("COUR_APPEL", "sn_cour_appel_dakar".to_string(), true))
        );
        assert_eq!(
            court_identity("ohada", "cour.commune.justice.arbitrage.(ohada)", &[]),
            Some(("CCJA", "ohada_ccja".to_string(), false))
        );
        assert_eq!(
            court_identity("sn", "tribunal.grande.instance.hors.classe.dakar", &[]),
            Some((
                "TRIBUNAL_PREMIERE_INSTANCE",
                "sn_tribunal_premiere_instance_hors_classe_dakar".to_string(),
                true
            ))
        );
        assert_eq!(
            court_identity("ro", "haute.cour.cassation.justice", &[]),
            Some((
                "COUR_CASSATION",
                "ro_cour_cassation_justice".to_string(),
                true
            ))
        );
        assert_eq!(
            court_identity("ad", "tribunal.superieur.justice", &[]),
            Some(("COUR_SUPREME", "ad_cour_supreme".to_string(), false))
        );
        assert_eq!(
            court_identity("ci", "cour.supreme.cote.ivoire", &["cote", "ivoire"]),
            Some(("COUR_SUPREME", "ci_cour_supreme".to_string(), false))
        );
        assert_eq!(court_identity("mc", "juge.tutelaire", &[]), None);
        assert_eq!(country_code("CADHP"), Some("unionafricaine"));
        assert_eq!(
            court_identity(
                "unionafricaine",
                &plain_court("cour.africaine.des.droits.de.l'homme.et.des.peuples"),
                &[]
            ),
            Some((
                "COUR_DROITS_HOMME",
                "unionafricaine_cour_droits_homme".to_string(),
                false
            ))
        );
        assert_eq!(
            court_identity("ne", "cour.etat", &["niger"]),
            Some(("COUR_SUPREME", "ne_cour_supreme_etat".to_string(), true))
        );
        assert_eq!(
            court_identity("ne", "cour.supreme", &["niger"]),
            Some(("COUR_SUPREME", "ne_cour_supreme".to_string(), false))
        );
        assert_eq!(plain_court("cour.d'appel.de.cotonou"), "cour.appel.cotonou");
        assert_eq!(
            plain_court("Cour.d'appel.du.littoral"),
            "cour.appel.littoral"
        );
        assert_eq!(
            plain_court("la.haute.cour.constitutionelle.du.madagascar"),
            "haute.cour.constitutionelle.madagascar"
        );
        assert_eq!(country_words("Côte d'Ivoire"), ["cote", "ivoire"]);
    }
}
