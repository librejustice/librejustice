//! Strip HTML → texte au bord I/O.
//!
//! Les corps CEDH (DOCX converti en HTML) et CJUE (resource xhtml/html) sont du
//! HTML born-digital. Le décodage de format reste dans `lj-sources` : ce module
//! aplatit le HTML en texte (retrait des balises, décodage des entités, collapse
//! du whitespace) pour que le parser pur de `lj-core` reçoive du texte, pas du
//! HTML à décoder. Strip plat, sans re-parse des sections CSS.

use regex::Regex;
use std::sync::OnceLock;

/// Aplatit un corps HTML/xhtml en texte FR : retire `<script>`/`<style>` avec
/// leur contenu, mappe les balises de bloc/`<br>` sur des sauts de ligne, retire
/// les balises restantes, décode les entités, puis collapse le whitespace.
pub fn strip_html(html: &str) -> String {
    static SCRIPT_STYLE_RE: OnceLock<Regex> = OnceLock::new();
    static BLOCK_RE: OnceLock<Regex> = OnceLock::new();
    static TAG_RE: OnceLock<Regex> = OnceLock::new();

    let script_style = SCRIPT_STYLE_RE.get_or_init(|| {
        Regex::new(r"(?is)<(script|style)\b[^>]*>.*?</(script|style)>")
            .expect("regex script/style valide")
    });
    // Balises dont la fermeture (ou l'auto-fermeture `<br/>`) marque une coupure
    // de ligne : sans ça le strip plat colle deux paragraphes en un seul mot.
    let block = BLOCK_RE.get_or_init(|| {
        Regex::new(r"(?is)<br\s*/?>|</(p|div|li|tr|h[1-6]|section|article|blockquote|td|th)\s*>")
            .expect("regex balises de bloc valide")
    });
    let tag = TAG_RE.get_or_init(|| Regex::new(r"(?s)<[^>]+>").expect("regex tag valide"));

    let no_script = script_style.replace_all(html, " ");
    let with_breaks = block.replace_all(&no_script, "\n");
    let no_tags = tag.replace_all(&with_breaks, "");
    let decoded = decode_entities(&no_tags);
    collapse_whitespace(&decoded)
}

/// Décode les entités HTML nommées courantes + numériques (`&#160;`, `&#xA0;`),
/// itéré jusqu'au point fixe (borné) : certains corps arrivent double-encodés
/// (`&amp;#8217;`) et un seul passage laisse une entité. Une entité inconnue est
/// laissée telle quelle.
pub fn decode_entities(s: &str) -> String {
    let mut courant = decode_entities_once(s);
    for _ in 0..2 {
        let suivant = decode_entities_once(&courant);
        if suivant == courant {
            break;
        }
        courant = suivant;
    }
    courant
}

/// A numeric reference in 0x80-0x9F names its Windows-1252 character, as HTML
/// reads it: `&#156;` is œ, not a control character.
const WINDOWS_1252: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

/// A code point of Word's Symbol font, the private area 0xF000-0xF0FF, where
/// a pasted list keeps its bullets, which no font renders.
fn caractere(cp: u32) -> Option<char> {
    match cp {
        0x80..=0x9f => Some(WINDOWS_1252[(cp - 0x80) as usize]),
        0xf000..=0xf0ff => Some('\u{2022}'),
        _ => char::from_u32(cp),
    }
}

/// A named reference of the WHATWG table, which is sorted by name.
fn named_entity(name: &str) -> Option<&'static str> {
    let table = &html_escape::NAMED_ENTITIES;
    table
        .binary_search_by(|(n, _)| n.cmp(&name.as_bytes()))
        .ok()
        .map(|i| table[i].1)
}

/// A text `strip_html` wrote, with the references it left undecoded decoded
/// as it decodes them now.
pub fn decode_stripped(text: &str) -> String {
    collapse_whitespace(&decode_entities(text))
}

fn decode_entities_once(s: &str) -> String {
    static ENTITY_RE: OnceLock<Regex> = OnceLock::new();
    let re = ENTITY_RE.get_or_init(|| {
        Regex::new(r"&(#x?[0-9A-Fa-f]+|[A-Za-z][A-Za-z0-9]*);").expect("regex entité valide")
    });
    re.replace_all(s, |caps: &regex::Captures| {
        let body = &caps[1];
        if let Some(num) = body.strip_prefix('#') {
            let cp = if let Some(hex) = num.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                num.parse::<u32>().ok()
            };
            return cp
                .and_then(caractere)
                .map(String::from)
                .unwrap_or_else(|| caps[0].to_string());
        }
        match body {
            // A soft hyphen is invisible and carries nothing in plain text.
            "shy" => String::new(),
            _ => named_entity(body).map_or_else(|| caps[0].to_string(), str::to_string),
        }
    })
    .into_owned()
}

/// Collapse le whitespace : chaque run d'espaces (y compris `\u{a0}` insécable
/// et fines, issus des entités) devient un espace simple ; chaque ligne est
/// trimée ; les lignes vides sont retirées et les non-vides jointes par `\n`.
fn collapse_whitespace(s: &str) -> String {
    static WS_RE: OnceLock<Regex> = OnceLock::new();
    let ws = WS_RE
        .get_or_init(|| Regex::new(r"[ \t\r\u{a0}\u{2009}\u{202f}]+").expect("regex ws valide"));

    s.split('\n')
        .map(|raw_line| ws.replace_all(raw_line, " ").trim().to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_keeps_french_text() {
        let html =
            "<html><body><p>Arr\u{ea}t de la Cour</p><p>Renvoi pr\u{e9}judiciel</p></body></html>";
        let out = strip_html(html);
        assert_eq!(out, "Arrêt de la Cour\nRenvoi préjudiciel");
    }

    #[test]
    fn decodes_named_and_numeric_entities() {
        let html = "<p>droit &agrave; un proc&egrave;s &laquo;&nbsp;\u{e9}quitable&nbsp;&raquo; &amp; libre</p>";
        let out = strip_html(html);
        assert_eq!(out, "droit à un procès « équitable » & libre");
    }

    #[test]
    fn every_named_reference_of_the_html_table_decodes() {
        let html = "<p>&Ouml;ffentlicher Dienst, Stra&szlig;e, Jo&atilde;o, n&ordm;&nbsp;3, Pr&shy;&eacute;sident, &unknown;</p>";
        assert_eq!(
            strip_html(html),
            "Öffentlicher Dienst, Straße, João, nº 3, Président, &unknown;"
        );
        assert_eq!(
            decode_stripped("AVOCAT G&Eacute;N&Eacute;RAL\nM&uuml;ller&thinsp;&amp;&thinsp;Co"),
            "AVOCAT GÉNÉRAL\nMüller & Co"
        );
    }

    #[test]
    fn decodes_numeric_entities_dec_and_hex() {
        let out = strip_html("<p>article &#167; et &#xA7; bis</p>");
        assert_eq!(out, "article § et § bis");
    }

    #[test]
    fn a_windows_1252_reference_and_a_symbol_font_bullet_read_as_text() {
        assert_eq!(
            decode_entities("c&#156;ur &#150; &#149; lot&#61607;1"),
            "c\u{153}ur \u{2013} \u{2022} lot\u{2022}1"
        );
    }

    #[test]
    fn decodes_uppercase_ligature_and_double_encoded_entities() {
        let out = strip_html(
            "<p>&Eacute;tude de ma&icirc;trise d&rsquo;&oelig;uvre &amp;#8217; &amp;Ocirc;</p>",
        );
        assert_eq!(out, "Étude de maîtrise d’œuvre ’ Ô");
    }

    #[test]
    fn drops_script_and_style_content() {
        let html = "<style>.x{color:red}</style><p>texte</p><script>var a=1;</script>";
        assert_eq!(strip_html(html), "texte");
    }

    #[test]
    fn collapses_whitespace_and_blank_lines() {
        let html = "<p>un   deux</p>\n\n\n<p>  trois  </p>";
        assert_eq!(strip_html(html), "un deux\ntrois");
    }

    #[test]
    fn br_becomes_newline() {
        assert_eq!(strip_html("a<br>b<br/>c"), "a\nb\nc");
    }

    #[test]
    fn unknown_entity_left_intact() {
        assert_eq!(strip_html("<p>a &fnord; b</p>"), "a &fnord; b");
    }

    #[test]
    fn empty_body_is_empty_string() {
        assert_eq!(strip_html("<html><body></body></html>"), "");
        assert_eq!(strip_html(""), "");
    }
}
