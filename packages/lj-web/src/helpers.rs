//! Helpers de composition + formatters d'affichage.
//!
//! - `cn()` : concat de classes Tailwind statiques.
//! - formatters : juridiction, score, comptes, dates FR. Logique pure (pas
//!   d'I/O) ; les libelles longs viennent des enums DTO.

use lj_dtos::{FacetTag, JurisdictionType};
use lj_vocab::{zone_est_departement, ZONE_FRANCE};

/// Concatene des fragments de classes Tailwind : trim, drop des vides, join sur un
/// espace.
pub fn cn<'a, I: IntoIterator<Item = &'a str>>(classes: I) -> String {
    classes
        .into_iter()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Libelle long d'un ordre de juridiction.
pub fn format_juridiction(jur: JurisdictionType) -> &'static str {
    match jur {
        JurisdictionType::Ta => "Tribunal administratif",
        JurisdictionType::Caa => "Cour administrative d'appel",
        JurisdictionType::Ce => "Conseil d'État",
        JurisdictionType::Constit => "Conseil constitutionnel",
        JurisdictionType::Tc => "Tribunal des conflits",
        JurisdictionType::Cc => "Cour de cassation",
        JurisdictionType::Ca => "Cour d'appel",
        JurisdictionType::Tj => "Tribunal judiciaire",
        JurisdictionType::Tcom => "Tribunal de commerce",
        JurisdictionType::Cph => "Conseil de prud'hommes",
        JurisdictionType::Assises => "Cour d'assises",
        JurisdictionType::Ccd => "Cour criminelle départementale",
        JurisdictionType::Cedh => "Cour européenne des droits de l'homme",
        JurisdictionType::Cjue => "Cour de justice de l'Union européenne",
        JurisdictionType::Cnda => "Cour nationale du droit d'asile",
        JurisdictionType::Cnil => "Commission nationale de l'informatique et des libertés",
        JurisdictionType::Ddd => "Défenseur des droits",
        JurisdictionType::Adlc => "Autorité de la concurrence",
        JurisdictionType::Amf => "Autorité des marchés financiers",
        JurisdictionType::Comeu => "Commission européenne",
        JurisdictionType::CourSupreme => "Cour suprême",
        JurisdictionType::CourCassation => "Cour de cassation",
        JurisdictionType::CourConstitutionnelle => "Cour constitutionnelle",
        JurisdictionType::CourAppel => "Cour d'appel",
        JurisdictionType::CourAdministrative => "Cour administrative",
        JurisdictionType::TribunalAdministratif => "Tribunal administratif",
        JurisdictionType::TribunalPremiereInstance => "Tribunal de première instance",
        JurisdictionType::TribunalCommerce => "Tribunal de commerce",
        JurisdictionType::CourCriminelle => "Cour criminelle",
        JurisdictionType::CourJustice => "Cour de justice",
        JurisdictionType::Ccja => "Cour commune de justice et d'arbitrage",
        JurisdictionType::CourDroitsHomme => "Cour des droits de l'homme",
        JurisdictionType::AutoriteAdministrative => "Autorité administrative",
    }
}

/// Nom de juridiction nettoye, ou `None` si vide apres normalisation.
fn sanitize_jurisdiction_name(value: Option<&str>) -> Option<String> {
    let normalized = value.map(str::trim).filter(|s| !s.is_empty())?;
    // `\s+,` -> `,` : recolle une virgule precedee d'espaces.
    let mut out = String::with_capacity(normalized.len());
    let chars: Vec<char> = normalized.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            // Lookahead : sequence d'espaces suivie d'une virgule -> on saute les
            // espaces et on n'emet que la virgule.
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && chars[j] == ',' {
                out.push(',');
                i = j + 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    // Coupe `, <Nre> chambre…` et `, juge des référés…` en fin de chaine.
    let cut = strip_trailing_chamber(&out);
    let cut = strip_trailing_refere(cut);
    let trimmed = cut.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Retire un suffixe `, [N(re|ère|e)] chambre…` (insensible a la casse), porte
/// de la regex `/,\s*(\d{1,2}(?:re|ère|e)?\s+chambre.*)$/i`.
fn strip_trailing_chamber(s: &str) -> &str {
    if let Some(comma) = s.rfind(',') {
        let tail = s[comma + 1..].trim_start();
        let lower = tail.to_lowercase();
        // Doit commencer par 1-2 chiffres puis (optionnel) re|ère|e, espaces, "chamber".
        if matches_chamber(&lower) {
            return s[..comma].trim_end();
        }
    }
    s
}

fn matches_chamber(lower: &str) -> bool {
    let bytes: Vec<char> = lower.chars().collect();
    let mut i = 0;
    let mut digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() && digits < 2 {
        i += 1;
        digits += 1;
    }
    if digits == 0 {
        return false;
    }
    for suffix in ["re", "ère", "e"] {
        if lower[byte_offset(&bytes, i)..].starts_with(suffix) {
            i += suffix.chars().count();
            break;
        }
    }
    let rest = &lower[byte_offset(&bytes, i)..];
    let rest = rest.trim_start();
    rest.starts_with("chamber")
}

fn byte_offset(chars: &[char], char_idx: usize) -> usize {
    chars[..char_idx].iter().map(|c| c.len_utf8()).sum()
}

/// Retire un suffixe `, juge des référés…`, porte de
/// `/,\s*(juge des référés.*)$/i`.
fn strip_trailing_refere(s: &str) -> &str {
    if let Some(comma) = s.rfind(',') {
        let tail = s[comma + 1..].trim_start().to_lowercase();
        if tail.starts_with("juge des référés") {
            return s[..comma].trim_end();
        }
    }
    s
}

/// Nom affichable pour une decision : nom de juridiction nettoye, sinon libelle
/// long du type.
pub fn format_decision_jurisdiction(jur: JurisdictionType, name: Option<&str>) -> String {
    sanitize_jurisdiction_name(name).unwrap_or_else(|| format_juridiction(jur).to_string())
}

/// Variante courte : nom nettoye (sinon libelle long du type) abrege par la
/// table de citation partagee (`lj_dtos::abbreviate_jurisdiction_name`).
pub fn format_short_decision_jurisdiction(jur: JurisdictionType, name: Option<&str>) -> String {
    let name =
        sanitize_jurisdiction_name(name).unwrap_or_else(|| format_juridiction(jur).to_string());
    lj_dtos::abbreviate_jurisdiction_name(&name)
}

/// Décompose un uid d'entité namespacé (`siren:552043002`) en
/// `(namespace, id local)`. Sans `:`, le namespace est vide et l'uid entier est
/// pris pour id local.
pub fn split_entity_uid(uid: &str) -> (&str, &str) {
    uid.split_once(':').unwrap_or(("", uid))
}

/// Score formate a 3 decimales.
pub fn format_score(score: f64) -> String {
    format!("{score:.3}")
}

/// Pagination shape of the THREE search surfaces, decisions, texts and notices:
/// ten results a page, ten pages. The resulting reach, 100, is the `MAX_RESULTS`
/// of the decision engine, the only one of the three bounded by its ranking; the
/// other two could go further and follow the same shape. The directory listing
/// is out of it, a catalogue one walks rather than a ranking one reads.
///
/// The page cap holds in browse mode too, where the total runs far past it:
/// `key_from_map` brings any page beyond back to the tenth, so without the cap
/// the pagination would offer pages that loop back on themselves.
pub const SEARCH_PAGE_SIZE: u32 = 10;
pub const SEARCH_MAX_PAGES: i64 = 10;

/// Encode un terme de requete pour le query string.
pub fn encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for b in query.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `n` with its thousands grouped.
pub fn group_thousands(ui: &crate::i18n::NumbersUi, n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(ui.thousands);
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Nombre de pages de pagination d'une surface de recherche.
pub fn total_pages(total: i64) -> i64 {
    let taille = i64::from(SEARCH_PAGE_SIZE);
    (total.min(taille * SEARCH_MAX_PAGES) + taille - 1) / taille
}

/// Numéro d'article en graphie française : la forme compacte LEGI colle le préfixe
/// de partie au numéral (« L1142-1 », « R123-4 », « D45 ») et la clé publique le
/// plie en minuscules (« l1142-1 ») ; la convention des juristes l'écrit
/// « L. 1142-1 ». Seul un préfixe lettre unique L/R/D/A directement suivi d'un
/// chiffre est réécrit — les autres formes (numéraux nus, « LO119 », étoilés
/// « R*011 », libellés « Annexe V ») passent telles quelles.
pub fn format_article_num(num: &str) -> String {
    let mut chars = num.chars();
    match (chars.next(), chars.next()) {
        (Some(p), Some(d))
            if matches!(p.to_ascii_uppercase(), 'L' | 'R' | 'D' | 'A') && d.is_ascii_digit() =>
        {
            format!("{}. {}", p.to_ascii_uppercase(), &num[1..])
        }
        _ => num.to_string(),
    }
}

/// An ISO `YYYY-MM-DD` date written in full; any other value as it came.
pub fn format_date(ui: &crate::i18n::DatesUi, value: &str) -> String {
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return value.to_string();
    };
    match (year.parse(), month.parse(), day.parse()) {
        (Ok(year), Ok(month @ 1..=12), Ok(day @ 1..=31)) => (ui.long_date)(year, month, day),
        _ => value.to_string(),
    }
}

/// `text` with the court's French name, `french`, and its chambers' put in `named`'s language.
pub fn renamed(named: &lj_dtos::CourtName, text: &str, french: Option<&str>) -> String {
    let mut out = match french {
        Some(french) => text.replacen(french, &named.name, 1),
        None => text.to_string(),
    };
    for (french, name) in &named.chambers {
        out = out.replacen(french.as_str(), name, 1);
    }
    out
}

/// A title the API composed, its French date written as `ui` writes dates.
pub fn retitle(ui: &crate::i18n::DatesUi, title: &str, date: Option<&str>) -> String {
    match date.map(str::trim).filter(|d| !d.is_empty()) {
        Some(date) => title.replacen(&composed_date(date), &format_date(ui, date), 1),
        None => title.to_string(),
    }
}

/// A date as the API writes it in a title, the first of a month in figures: `1 février 2024`.
fn composed_date(date: &str) -> String {
    let french = format_date(&crate::i18n::Lang::Fr.ui().dates, date);
    match french.strip_prefix("1er ") {
        Some(rest) => format!("1 {rest}"),
        None => french,
    }
}

/// A date that may be missing or blank.
pub fn format_iso_date(ui: &crate::i18n::DatesUi, value: Option<&str>) -> String {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => format_date(ui, value),
        None => ui.undated.to_string(),
    }
}

/// Today, ISO: the browser's own day, Paris's on the server.
#[cfg(feature = "hydrate")]
pub fn today_iso() -> String {
    let d = js_sys::Date::new_0();
    format!(
        "{:04}-{:02}-{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date()
    )
}
#[cfg(not(feature = "hydrate"))]
pub fn today_iso() -> String {
    jiff::Zoned::now()
        .in_tz("Europe/Paris")
        .expect("the tz database is bundled")
        .date()
        .to_string()
}

/// `/decision-search?{key}={value}…`, escaped by `ParamsMap`, where a key given twice
/// adds a value instead of replacing it: that is how a multi-valued facet passes. The
/// path is written from the jurisdiction, which `ReaderScope::href` puts in front.
pub fn decisions_href(params: impl IntoIterator<Item = (&'static str, String)>) -> String {
    engine_href("/decision-search", params)
}

/// `/norm-search?{key}={value}…`, on the terms of [`decisions_href`].
pub fn norms_href(params: impl IntoIterator<Item = (&'static str, String)>) -> String {
    engine_href("/norm-search", params)
}

fn engine_href(path: &str, params: impl IntoIterator<Item = (&'static str, String)>) -> String {
    let mut qs = leptos_router::params::ParamsMap::new();
    for (key, value) in params {
        qs.insert(key, value);
    }
    format!("{path}{}", qs.to_query_string())
}

/// What a market's zone list reads as on one line. A nationwide market carries
/// every département rather than `fr`, so its first code says no more about
/// where it is open than any other: 107 of the open markets read « Ain » that
/// way. `None` when no zone is a département.
pub fn portee_zones(ui: &crate::i18n::NoticesUi, zones: &[FacetTag]) -> Option<String> {
    if zones.iter().any(|z| z.key == ZONE_FRANCE) {
        return Some(ui.whole_country.to_string());
    }
    let deps: Vec<&FacetTag> = zones
        .iter()
        .filter(|z| zone_est_departement(&z.key))
        .collect();
    match deps.as_slice() {
        [] => None,
        [seul] => Some(seul.label.clone()),
        plusieurs => Some((ui.department_count)(plusieurs.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::{format_article_num, portee_zones, retitle};
    use crate::i18n::Lang;
    use lj_dtos::FacetTag;

    #[test]
    fn a_title_takes_the_reader_s_date() {
        let title = "Cour d'appel (Bâle-Ville, Suisse), 1 février 2024, VD.2024.111";
        assert_eq!(
            retitle(&Lang::De.ui().dates, title, Some("2024-02-01")),
            "Cour d'appel (Bâle-Ville, Suisse), 1. Februar 2024, VD.2024.111"
        );
        assert_eq!(
            retitle(&Lang::Fr.ui().dates, title, Some("2024-02-01")),
            "Cour d'appel (Bâle-Ville, Suisse), 1er février 2024, VD.2024.111"
        );
    }

    fn tag(key: &str, label: &str) -> FacetTag {
        FacetTag {
            key: key.into(),
            label: label.into(),
        }
    }

    #[test]
    fn un_marche_national_ne_se_lit_pas_par_son_premier_departement() {
        let france: Vec<FacetTag> = ["01", "973", "75"]
            .iter()
            .map(|c| tag(c, "peu importe"))
            .collect();
        let ui = &Lang::Fr.ui().notices;
        assert_eq!(portee_zones(ui, &france).as_deref(), Some("3 départements"));
        assert_eq!(
            portee_zones(ui, &[tag("fr", "France entière"), tag("01", "Ain")]).as_deref(),
            Some("France entière")
        );
        assert_eq!(
            portee_zones(ui, &[tag("973", "Guyane")]).as_deref(),
            Some("Guyane")
        );
        assert_eq!(portee_zones(ui, &[tag("sans", "Non précisé")]), None);
    }

    #[test]
    fn format_article_num_dots_single_letter_prefixes_only() {
        assert_eq!(format_article_num("L1142-1"), "L. 1142-1");
        assert_eq!(format_article_num("R123-4"), "R. 123-4");
        assert_eq!(format_article_num("D45"), "D. 45");
        assert_eq!(format_article_num("A412-1"), "A. 412-1");
        assert_eq!(format_article_num("1240"), "1240");
        assert_eq!(format_article_num("LO119"), "LO119");
        assert_eq!(format_article_num("R*011"), "R*011");
        assert_eq!(format_article_num("LP. 974-2"), "LP. 974-2");
        assert_eq!(format_article_num("Annexe V"), "Annexe V");
    }
}
