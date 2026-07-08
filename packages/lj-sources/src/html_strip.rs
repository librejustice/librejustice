//! Strip HTML → texte au bord I/O (ADR 0094 §Frontière pure ↔ I/O).
//!
//! Les corps CEDH (DOCX converti en HTML) et CJUE (resource xhtml/html) sont
//! du HTML born-digital. Le décodage de format reste dans `lj-sources` : ce
//! module aplatit le HTML en texte (retrait des balises, décodage des entités,
//! collapse du whitespace) pour que le parser pur de `lj-core` reçoive du texte,
//! pas du HTML à décoder. Strip plat, sans re-parse des sections CSS.

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

/// Décode les entités HTML nommées courantes + numériques (`&#160;`, `&#xA0;`).
/// Couvre ce qui apparaît réellement dans les corps CEDH/CJUE convertis ; une
/// entité inconnue est laissée telle quelle (pas de panique : strip best-effort).
fn decode_entities(s: &str) -> String {
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
                .and_then(char::from_u32)
                .map(String::from)
                .unwrap_or_else(|| caps[0].to_string());
        }
        match body {
            "amp" => "&".to_string(),
            "lt" => "<".to_string(),
            "gt" => ">".to_string(),
            "quot" => "\"".to_string(),
            "apos" => "'".to_string(),
            "nbsp" => "\u{a0}".to_string(),
            "laquo" => "\u{ab}".to_string(),
            "raquo" => "\u{bb}".to_string(),
            "ndash" => "\u{2013}".to_string(),
            "mdash" => "\u{2014}".to_string(),
            "lsquo" => "\u{2018}".to_string(),
            "rsquo" => "\u{2019}".to_string(),
            "ldquo" => "\u{201c}".to_string(),
            "rdquo" => "\u{201d}".to_string(),
            "hellip" => "\u{2026}".to_string(),
            "eacute" => "é".to_string(),
            "egrave" => "è".to_string(),
            "ecirc" => "ê".to_string(),
            "agrave" => "à".to_string(),
            "acirc" => "â".to_string(),
            "ccedil" => "ç".to_string(),
            "ugrave" => "ù".to_string(),
            "ucirc" => "û".to_string(),
            "icirc" => "î".to_string(),
            "iuml" => "ï".to_string(),
            "ocirc" => "ô".to_string(),
            "euml" => "ë".to_string(),
            "deg" => "\u{b0}".to_string(),
            "euro" => "\u{20ac}".to_string(),
            "sect" => "\u{a7}".to_string(),
            _ => caps[0].to_string(),
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
        // Les entités sont décodées (`&agrave;`→à, `&laquo;`→«, `&amp;`→&) ; le
        // `&nbsp;` décodé en U+00A0 est ensuite collapsé en espace simple comme
        // tout whitespace.
        let html = "<p>droit &agrave; un proc&egrave;s &laquo;&nbsp;\u{e9}quitable&nbsp;&raquo; &amp; libre</p>";
        let out = strip_html(html);
        assert_eq!(out, "droit à un procès « équitable » & libre");
    }

    #[test]
    fn decodes_numeric_entities_dec_and_hex() {
        // &#167; = §, &#xA7; = § aussi.
        let out = strip_html("<p>article &#167; et &#xA7; bis</p>");
        assert_eq!(out, "article § et § bis");
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
