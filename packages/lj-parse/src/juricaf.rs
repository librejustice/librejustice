//! Parse d'une page « arrêt » JuriCAF (jurisprudence francophone, AHJUCAF) :
//! HTML → [`Decision`] + `source_fields`. Sonde de repérage du fond
//! international — l'identité vient de l'URN:LEX que JuriCAF grave dans
//! chaque page (`urn:lex;sn;cour.supreme;arret;2024-11-14;57`), le texte du
//! conteneur `id="arret"`.

use serde_json::{json, Value};

use lj_core::decision::Decision;
use lj_core::error::{CoreError, Result};

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

/// Date ISO depuis le segment date de l'URN:LEX. Trois graphies observées sur
/// le fond : `2024-11-14` (nominal), `27-12-2019` (jour en tête, Tchad),
/// `99-11-23` (année à deux chiffres, vieux Sénégal — toujours 19xx, le fond
/// numérique commence dans les années 1960).
fn urn_date_to_iso(seg: &str) -> Option<String> {
    let parts: Vec<&str> = seg.split('-').collect();
    let [a, b, c] = parts.as_slice() else {
        return None;
    };
    let (y, m, d) = match (a.len(), c.len()) {
        (4, _) => (a.to_string(), b, c),
        (_, 4) => (c.to_string(), b, a),
        (2, _) => (format!("19{a}"), b, c),
        _ => return None,
    };
    let (m, d): (u32, u32) = (m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(format!("{y}-{m:02}-{d:02}"))
}

/// Libellé français d'un segment juridiction URN:LEX (`cour.supreme` →
/// « Cour suprême »). Répertoire des cours du fond ; un segment inconnu se
/// title-case mécaniquement.
fn court_label(court: &str) -> String {
    match court {
        "cour.supreme" => "Cour suprême".to_string(),
        "cour.cassation" => "Cour de cassation".to_string(),
        "cour.appel" => "Cour d'appel".to_string(),
        "conseil.constitutionnel" => "Conseil constitutionnel".to_string(),
        "cour.justice" => "Cour de justice".to_string(),
        c if c.starts_with("cour.commune.justice.arbitrage") => {
            "Cour commune de justice et d'arbitrage (CCJA)".to_string()
        }
        c if c.starts_with("tribunal.grande.instance") => {
            let mut label = "Tribunal de grande instance".to_string();
            if let Some(rest) = c.strip_prefix("tribunal.grande.instance.") {
                label.push(' ');
                label.push_str(&rest.replace('.', " "));
            }
            label
        }
        c => {
            let mut out = String::new();
            for (i, w) in c.split('.').enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                let mut chars = w.chars();
                if let Some(f) = chars.next() {
                    if i == 0 {
                        out.extend(f.to_uppercase());
                    } else {
                        out.push(f);
                    }
                    out.push_str(chars.as_str());
                }
            }
            out
        }
    }
}

/// Nom français du pays/l'organisation depuis le segment pays URN:LEX.
fn country_label(iso: &str) -> &'static str {
    match iso {
        "sn" => "Sénégal",
        "gn" => "Guinée",
        "ht" => "Haïti",
        "lb" => "Liban",
        "ma" => "Maroc",
        "td" => "Tchad",
        "tg" => "Togo",
        "bj" => "Bénin",
        "mc" => "Monaco",
        "mg" => "Madagascar",
        "ml" => "Mali",
        "ne" => "Niger",
        "tn" => "Tunisie",
        "ci" => "Côte d'Ivoire",
        "bf" => "Burkina Faso",
        "cm" => "Cameroun",
        "ga" => "Gabon",
        "cd" => "RD Congo",
        "be" => "Belgique",
        "lu" => "Luxembourg",
        "ch" => "Suisse",
        "ca" => "Canada",
        "ad" => "Andorre",
        "ohada" => "OHADA",
        "uemoa" => "UEMOA",
        _ => "",
    }
}

/// The foreign court types, by the leading words of the URN:LEX court segment.
const COURT_FAMILIES: [(&[&str], &str); 21] = [
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
    (&["tribunal", "commerce"], "TRIBUNAL_COMMERCE"),
    (&["cour", "criminelle"], "COUR_CRIMINELLE"),
    (&["cour", "justice"], "COUR_JUSTICE"),
    (&["cour", "commune", "justice", "arbitrage"], "CCJA"),
    (
        &["cour", "africaine", "droits", "homme"],
        "COUR_DROITS_HOMME",
    ),
    (
        &["cour", "interamericaine", "droits", "homme"],
        "COUR_DROITS_HOMME",
    ),
];

/// The court segment without its elisions and linking words, as most URNs already write it.
fn plain_court(court: &str) -> String {
    court
        .split('.')
        .map(|w| w.strip_prefix("d'").unwrap_or(w))
        .filter(|w| !matches!(*w, "de" | "du" | "des"))
        .collect::<Vec<_>>()
        .join(".")
}

/// A parenthesised word of the court segment repeats the organisation.
fn court_identity(country: &str, court: &str) -> Option<(&'static str, String)> {
    let words: Vec<&str> = court.split('.').filter(|w| !w.starts_with('(')).collect();
    let (head, kind) = COURT_FAMILIES
        .iter()
        .filter(|(head, _)| words.starts_with(head))
        .max_by_key(|(head, _)| head.len())?;
    let code = lj_vocab::courts::foreign_court_code(country, kind, &words[head.len()..]);
    Some((kind, code))
}

/// `None` when the court is of no foreign type.
fn decision_from_urn(
    slug: &str,
    country: &str,
    court: &str,
    date_seg: &str,
    num: &str,
    text: String,
) -> Option<Decision> {
    let court = plain_court(court);
    let (kind, code) = court_identity(country, &court)?;
    let numero = num.replace('.', "-");
    let country_lbl = country_label(country);
    let court_lbl = court_label(&court);
    let jurisdiction_label = if country_lbl.is_empty() {
        court_lbl.clone()
    } else {
        format!("{court_lbl} ({country_lbl})")
    };
    Some(Decision {
        source_uid: format!("juricaf/{slug}"),
        member_name: slug.to_string(),
        ecli: None,
        jurisdiction_source_code: Some(code),
        chamber: None,
        nac: None,
        jurisdiction_name: Some(jurisdiction_label),
        jurisdiction_type: Some(kind.to_string()),
        jurisdiction_location: None,
        numero_dossier: Some(numero.clone()),
        numero_dossiers: Some(vec![numero]),
        numero_role: None,
        date_lecture: urn_date_to_iso(date_seg),
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

/// Branche JuriCAF de [`Decision::from_source_fields`] : re-dérive la
/// `Decision` depuis l'URN:LEX gravé en `source_fields` au parse. L'URN a été
/// validé (5 segments) à la frontière `parse_juricaf` — en aval on fait confiance
/// au stockage, un URN cassé est une erreur franche.
pub(crate) fn from_source_fields_juricaf(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let urn = source_fields
        .get("urn_lex")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("source_fields juricaf sans urn_lex: {source_uid}"));
    let segs: Vec<&str> = urn.split(';').collect();
    let [country, court, _nature, date_seg, num] = segs.as_slice() else {
        panic!("source_fields juricaf: URN:LEX malformé {urn} ({source_uid})");
    };
    let slug = source_uid.strip_prefix("juricaf/").unwrap_or(source_uid);
    decision_from_urn(slug, country, court, date_seg, num, full_text.to_string()).unwrap_or_else(
        || panic!("stored juricaf court of no foreign type: {court} ({source_uid})"),
    )
}

/// Parse une page « arrêt » JuriCAF. `slug` = segment d'URL après `/arret/`
/// (`SENEGAL-COURSUPREME-20241114-57`), pivot du `source_uid`.
/// The country a page declares in `dc.coverage`, the name JuriCAF's country feed takes.
pub fn juricaf_country(raw: &[u8]) -> Option<String> {
    meta_content(&decode_page(raw), "dc.coverage").map(str::to_string)
}

pub fn parse_juricaf(slug: &str, raw: &[u8]) -> Result<JuricafParsed> {
    let html = decode_page(raw);

    let urn = html
        .split("urn:lex;")
        .nth(1)
        .and_then(|rest| {
            rest.split(|c: char| c == '"' || c == '<' || c.is_whitespace())
                .next()
        })
        .ok_or_else(|| CoreError::Xml(format!("juricaf {slug}: URN:LEX introuvable")))?
        .trim_end_matches(&[';', '.'][..])
        .to_string();
    let segs: Vec<&str> = urn.split(';').collect();
    let [country, court, _nature, date_seg, num] = segs.as_slice() else {
        return Err(CoreError::Xml(format!(
            "juricaf {slug}: URN:LEX à {} segments (5 attendus) : {urn}",
            segs.len()
        )));
    };

    let title = html
        .split("<title>")
        .nth(1)
        .and_then(|r| r.split("</title>").next())
        .map(|t| unescape(t.trim()).to_string())
        .unwrap_or_else(|| slug.to_string());

    let start = html
        .find("id=\"arret\"")
        .ok_or_else(|| CoreError::Xml(format!("juricaf {slug}: conteneur arrêt introuvable")))?;
    let end = html[start..]
        .find("Origine de la décision")
        .map(|i| start + i)
        .unwrap_or(html.len());
    let text = strip_tags(&html[start..end]);
    if text.chars().filter(|c| !c.is_whitespace()).count() < 200 {
        return Err(CoreError::Xml(format!(
            "juricaf {slug}: texte de décision vide ou squelettique"
        )));
    }

    let language = meta_content(&html, "dc.language").unwrap_or_default();
    let lang = lj_vocab::languages::eu_of_iso_639_1(&language.to_ascii_lowercase())
        .ok_or_else(|| CoreError::Xml(format!("juricaf {slug}: page language {language:?}")))?;

    let source_fields = json!({
        "urn_lex": urn,
        "juricaf_slug": slug,
        "country": country,
        "court": court,
        "title": title,
        "lang": lang,
        "source_url": format!("https://juricaf.org/arret/{slug}"),
    });

    let decision =
        decision_from_urn(slug, country, court, date_seg, num, text).ok_or_else(|| {
            CoreError::Xml(format!("juricaf {slug}: court of no foreign type: {court}"))
        })?;
    let jurisdiction_label = decision
        .jurisdiction_name
        .clone()
        .expect("decision_from_urn pose toujours le nom");

    Ok(JuricafParsed {
        jurisdiction_code: decision
            .jurisdiction_source_code
            .clone()
            .expect("decision_from_urn codes the court"),
        decision,
        source_fields,
        jurisdiction_label,
    })
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

    #[test]
    fn urn_dates_all_three_shapes() {
        assert_eq!(urn_date_to_iso("2024-11-14").as_deref(), Some("2024-11-14"));
        assert_eq!(urn_date_to_iso("27-12-2019").as_deref(), Some("2019-12-27"));
        assert_eq!(urn_date_to_iso("99-11-23").as_deref(), Some("1999-11-23"));
        assert_eq!(urn_date_to_iso("garbage"), None);
    }

    #[test]
    fn parse_extracts_urn_identity_and_text() {
        let html = format!(
            "<html><head><title>Sénégal, Cour suprême, 14 novembre 2024, 57</title>\
             <meta content=\"urn:lex;sn;cour.supreme;arret;2024-11-14;57\">\
             <meta name=\"dc.language\" content=\"FR\" /></head>\
             <body><div id=\"arret\"><p>{}</p></div>Origine de la décision</body></html>",
            "LA COUR SUPRÊME, statuant en matière civile. ".repeat(10)
        );
        let p = parse_juricaf("SENEGAL-COURSUPREME-20241114-57", html.as_bytes()).unwrap();
        assert_eq!(p.jurisdiction_code, "sn_cour_supreme");
        assert_eq!(
            p.decision.jurisdiction_type.as_deref(),
            Some("COUR_SUPREME")
        );
        assert_eq!(p.decision.date_lecture.as_deref(), Some("2024-11-14"));
        assert_eq!(p.jurisdiction_label, "Cour suprême (Sénégal)");
        assert!(p.decision.texte_integral_clean.contains("COUR SUPRÊME"));
        assert_eq!(p.source_fields["lang"], "fra");
    }

    #[test]
    fn a_court_segment_opens_with_its_type_and_ends_with_its_name() {
        assert_eq!(
            court_identity("sn", "cour.appel.dakar"),
            Some(("COUR_APPEL", "sn_cour_appel_dakar".to_string()))
        );
        assert_eq!(
            court_identity("ohada", "cour.commune.justice.arbitrage.(ohada)"),
            Some(("CCJA", "ohada_ccja".to_string()))
        );
        assert_eq!(
            court_identity("sn", "tribunal.grande.instance.hors.classe.dakar"),
            Some((
                "TRIBUNAL_PREMIERE_INSTANCE",
                "sn_tribunal_premiere_instance_hors_classe_dakar".to_string()
            ))
        );
        assert_eq!(
            court_identity("ro", "haute.cour.cassation.justice"),
            Some(("COUR_CASSATION", "ro_cour_cassation_justice".to_string()))
        );
        assert_eq!(
            court_identity("ad", "tribunal.superieur.justice"),
            Some(("COUR_SUPREME", "ad_cour_supreme".to_string()))
        );
        assert_eq!(court_identity("mc", "juge.tutelaire"), None);
        assert_eq!(plain_court("cour.d'appel.de.cotonou"), "cour.appel.cotonou");
        assert_eq!(
            plain_court("cour.supreme.de.cassation"),
            "cour.supreme.cassation"
        );
    }
}
