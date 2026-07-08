//! Construction du prompt de résumé + nettoyage de sortie (port de `summary.py`).
//!
//! PUR : pas d'appel LLM ici (l'I/O réseau vit dans `lj-ingest`). On expose le
//! prompt versionné, le découpage tête/queue de l'input, et le nettoyage de la
//! sortie modèle.

use std::sync::LazyLock;

use regex::Regex;

/// Version du prompt persistée en DB (`decisions.summary_prompt_version`,
/// colonne `SMALLINT` → `i16`).
pub const SUMMARY_PROMPT_VERSION: i16 = 4;

/// Prompt système de résumé (texte intégral, source de vérité).
pub const SUMMARY_PROMPT: &str = include_str!("../data/summary_prompt.txt");

// --- Construction de l'entrée Mistral --------------------------------------
//
// mistral-small a 128k tokens de contexte (~400k c) ; la décision la plus longue
// du corpus ≈ 99k c → on envoie le texte INTÉGRAL par défaut. Au-delà du cap
// (rares géantes), on garde la TÊTE (parties, moyens, motifs) ET la QUEUE
// (dispositif) — jamais la tête seule, qui inverse l'issue.

const SUMMARY_INPUT_CAP: usize = 80_000;
const SUMMARY_INPUT_HEAD: usize = 56_000;
const SUMMARY_INPUT_TAIL: usize = 24_000;
const ELISION: &str = "\n[...]\n";

/// Borne l'input modèle : intégral si `≤ cap`, sinon tête + queue avec élision.
///
/// Les bornes Python (`body_text[:HEAD]`, `body_text[-TAIL:]`) sont des slices
/// sur des caractères Python (code points). On reproduit donc un découpage sur
/// frontières de caractères (et non d'octets) pour rester fidèle aux longueurs.
pub fn build_summary_input(body_text: &str) -> String {
    // `len(body_text)` en Python compte les code points.
    let char_len = body_text.chars().count();
    if char_len <= SUMMARY_INPUT_CAP {
        return body_text.to_string();
    }
    let head: String = body_text.chars().take(SUMMARY_INPUT_HEAD).collect();
    let tail: String = body_text
        .chars()
        .skip(char_len - SUMMARY_INPUT_TAIL)
        .collect();
    format!("{head}{ELISION}{tail}")
}

// --- Nettoyage déterministe des codes d'anonymisation ----------------------
//
// Le LLM recopie parfois la pseudonymisation de la source. Deux familles :
// - placeholders sans valeur de nom — lieu (« [Localité 6] ») ou code numérique
//   (« la société [7] », « [22] ») : on supprime le code ET sa préposition
//   introductrice éventuelle, sinon « le comité économique du [7] » laisse « le
//   comité économique du, » orphelin.
// - initiales d'une personne : « M. [W] [Y] » — c'est le nom anonymisé, on garde
//   le contenu et on retire juste les crochets → « M. W Y ».
//
// Le LLM recopie aussi parfois l'en-tête « [Décision] <jur>, <date>, <num> »
// (interdit par le prompt) en première ligne ; il pollue alors la meta
// description → on le retire.

/// En-tête recopié en tête de résumé : ligne « [Décision]/Décision … <date ISO>
/// … » suivie d'un saut de ligne. La date ISO (signature de la citation injectée
/// en en-tête) évite de confondre avec une vraie phrase commençant par « Décision ».
static HEADER_ECHO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[?\s*Décision\b[^\n]*?\d{4}-\d{2}-\d{2}[^\n]*\n+").unwrap());
/// Placeholder pseudonymisé sans valeur de nom — lieu ou code numérique — retiré
/// avec sa préposition introductrice éventuelle.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[ \t]*(?:\b(?:de la|de l'|du|des|de|à la|aux|au|à|en|dans|sur|vers|chez)\s*)?\[(?:(?:Localité|Adresse|Commune|Ville)[^\]]*|\d[^\]]*)\]",
    )
    .unwrap()
});
/// Initiales de personne « [W] » → contenu gardé, crochets retirés.
static INITIAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\d][^\]]{0,29})\]").unwrap());
static SPACE_BEFORE_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+([,.;:!?])").unwrap());
static MULTISPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
/// Ponctuation/espaces orphelins en tête (placeholder retiré en début de phrase).
static LEADING_JUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\s,;:.–—-]+").unwrap());

/// Nettoie la sortie modèle (echo d'en-tête, placeholders, initiales, ponctuation).
///
/// Trois choses : (1) un en-tête « [Décision] … » recopié en tête ; (2) les
/// placeholders (numéros, lieux) avec leur préposition introductrice ; en gardant
/// (3) les noms (initiales) — crochets retirés. Préserve les sauts de ligne
/// (séparateur de phrases) et est idempotent / no-op sur un résumé déjà propre.
pub fn clean_summary(text: &str) -> String {
    let text = HEADER_ECHO.replace(text, "");
    let text = PLACEHOLDER.replace_all(&text, "");
    let text = INITIAL.replace_all(&text, "$1");
    let text = SPACE_BEFORE_PUNCT.replace_all(&text, "$1");
    let text = MULTISPACE.replace_all(&text, " ");
    // `subn` Python : on a besoin de savoir si quelque chose a été retiré en tête.
    let leading_removed = LEADING_JUNK.is_match(&text);
    let text = LEADING_JUNK.replace(&text, "");
    let text = text.trim();

    // Un placeholder retiré en tête de phrase laisse espace/ponctuation orphelins
    // (`leading_removed`), puis une minuscule en première position : un résumé
    // ouvre toujours une phrase, on recapitalise. On ne touche pas un texte déjà
    // propre (fragments de test compris).
    if leading_removed && first_char_is_lower(text) {
        capitalize_first(text)
    } else {
        text.to_string()
    }
}

/// `text[:1].islower()` Python : vrai ssi le premier caractère est une minuscule
/// (au sens Unicode). Faux sur chaîne vide ou premier char non-cased.
fn first_char_is_lower(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_lowercase())
}

/// `text[0].upper() + text[1:]` Python : majuscule du premier caractère.
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_version_is_locked() {
        assert_eq!(SUMMARY_PROMPT_VERSION, 4);
        assert!(SUMMARY_PROMPT.starts_with("Tu es juriste."));
    }

    #[test]
    fn build_input_passthrough_under_cap() {
        let body = "petite décision";
        assert_eq!(build_summary_input(body), body);
    }

    #[test]
    fn build_input_head_tail_over_cap() {
        // Caractères ASCII : 1 char = 1 octet, longueurs Python = longueurs char.
        let body: String = "x".repeat(SUMMARY_INPUT_CAP + 1);
        let out = build_summary_input(&body);
        // tête + élision + queue
        assert_eq!(
            out.chars().count(),
            SUMMARY_INPUT_HEAD + ELISION.chars().count() + SUMMARY_INPUT_TAIL
        );
        assert!(out.contains("\n[...]\n"));
    }

    #[test]
    fn build_input_at_cap_is_passthrough() {
        let body: String = "y".repeat(SUMMARY_INPUT_CAP);
        assert_eq!(build_summary_input(&body), body);
    }

    #[test]
    fn clean_keeps_names_strips_brackets() {
        // « M. [W] [Y] » → « M. W Y »
        assert_eq!(
            clean_summary("M. [W] [Y] conteste le refus."),
            "M. W Y conteste le refus."
        );
    }

    #[test]
    fn clean_strips_placeholder_with_preposition() {
        // « le comité du [7] » → « le comité »
        assert_eq!(clean_summary("le comité du [7] décide"), "le comité décide");
        // « la cour de [Localité 5] » → « la cour »
        assert_eq!(
            clean_summary("la cour de [Localité 5] statue"),
            "la cour statue"
        );
    }

    #[test]
    fn clean_strips_locality_placeholder() {
        // « [Localité 6] » → rien
        assert_eq!(
            clean_summary("La société [Localité 6] agit."),
            "La société agit."
        );
    }

    #[test]
    fn clean_strips_header_echo() {
        let input =
            "[Décision] Cour de cassation, 2024-03-12, 22-10.123\nUn salarié conteste son licenciement.";
        assert_eq!(
            clean_summary(input),
            "Un salarié conteste son licenciement."
        );
    }

    #[test]
    fn clean_recapitalizes_after_leading_junk() {
        // placeholder retiré en tête → ponctuation orpheline → recapitalisation.
        assert_eq!(
            clean_summary("[7] saisit le tribunal."),
            "Saisit le tribunal."
        );
    }

    #[test]
    fn clean_is_noop_on_clean_summary() {
        let clean = "Un salarié licencié conteste la rupture de son contrat.";
        assert_eq!(clean_summary(clean), clean);
    }

    #[test]
    fn clean_removes_space_before_punct() {
        assert_eq!(clean_summary("le comité [7] , statue"), "le comité, statue");
    }
}
