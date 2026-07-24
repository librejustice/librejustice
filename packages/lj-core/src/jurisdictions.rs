//! Détection des juridictions étrangères nommées dans une requête de
//! recherche de textes. Le corpus normatif mélange droit applicable en France
//! (FR, UE, INTL) et codes étrangers francophones ou traduits (~60 pays) dont
//! la rédaction napoléonienne matche mot pour mot les requêtes françaises —
//! sans prior, « responsabilité du fait des choses » sort le code civil belge
//! avant l'article 1242. Le ranking déclasse donc l'étranger **sauf si la
//! requête nomme le pays** : cette table pays/gentilé → code ISO (celle du
//! champ `legal_text.jurisdiction`) porte la détection. Pur, sans I/O.

use crate::text::fold;

/// `code ISO` → variantes foldées (minuscules, sans accent). Convention :
/// suffixe `*` = préfixe de token (couvre les flexions du gentilé :
/// `senegal*` matche « sénégalais(e)(s) ») ; entrée à espace = sous-chaîne
/// bornée par mots ; sinon token exact (les formes courtes ambiguës — « mali »
/// ne doit pas matcher « malice »). Seuls les pays présents en corpus.
const COUNTRIES: &[(&str, &[&str])] = &[
    ("AM", &["armenie*", "armenien*"]),
    ("AO", &["angol*"]),
    ("AT", &["autriche*", "autrichien*"]),
    ("BE", &["belge*", "belgique"]),
    ("BF", &["burkina*"]),
    ("BG", &["bulgar*"]),
    ("BI", &["burundi", "burundais*"]),
    ("BJ", &["benin", "beninois*"]),
    ("CD", &["congo", "congolais*", "kinshasa", "rdc", "zaire*"]),
    ("CF", &["centrafri*"]),
    ("CG", &["congo", "congolais*", "brazzaville"]),
    ("CH", &["suisse*", "helvetique*"]),
    ("CI", &["ivoir*"]),
    ("CM", &["cameroun*"]),
    ("DE", &["allemagne", "allemand*"]),
    ("DJ", &["djibout*"]),
    ("DO", &["dominicain*"]),
    ("DZ", &["algerie*", "algerien*"]),
    ("EG", &["egypt*"]),
    ("ES", &["espagne*", "espagnol*"]),
    ("GA", &["gabon*"]),
    ("GN", &["guine*"]),
    ("GR", &["grec*"]),
    ("HT", &["haiti*", "haitien*"]),
    ("HU", &["hongrie*", "hongrois*"]),
    ("IQ", &["irak*", "iraq*"]),
    ("IT", &["italie*", "italien*"]),
    ("JO", &["jordanie*", "jordanien*"]),
    ("KM", &["comor*"]),
    ("LB", &["liban*"]),
    ("LU", &["luxembourg*"]),
    ("MA", &["maroc*"]),
    ("MC", &["monaco", "monegasque*"]),
    ("MG", &["madagascar", "malgache*"]),
    ("ML", &["mali", "malien*"]),
    ("MR", &["mauritanie*", "mauritanien*"]),
    ("MU", &["maurice", "mauricien*"]),
    ("NC", &["caledonie*", "caledonien*"]),
    ("NE", &["niger", "nigerien*"]),
    ("NG", &["nigeria*", "nigerian*"]),
    ("NL", &["neerlandais*", "pays bas"]),
    ("PE", &["perou", "peruvien*"]),
    ("PF", &["polynesie*", "polynesien*", "tahiti*"]),
    ("PL", &["pologne*", "polonais*"]),
    ("PT", &["portug*"]),
    ("RO", &["roumanie*", "roumain*"]),
    ("RS", &["serbie*", "serbe*"]),
    ("RU", &["russie*", "russe*"]),
    ("RW", &["rwand*"]),
    ("SN", &["senegal*"]),
    ("ST", &["sao tome", "santomeen*"]),
    ("SY", &["syrie*", "syrien*"]),
    ("TD", &["tchad*"]),
    ("TG", &["togo", "togolais*"]),
    ("TN", &["tunisie*", "tunisien*", "tunis"]),
    ("TR", &["turquie*", "turc", "turcs", "turque*"]),
    ("UA", &["ukraine*", "ukrainien*"]),
    ("VE", &["venezuel*"]),
    ("VN", &["vietnam*", "viet", "vietnamien*"]),
];

/// Codes ISO des juridictions étrangères que `query` nomme (pays ou gentilé),
/// dans l'ordre de la table. « congo(lais) » est ambigu → `CG` et `CD`.
/// Vide = requête domestique (le prior FR/UE/INTL s'applique seul).
pub fn query_jurisdictions(query: &str) -> Vec<&'static str> {
    strip_query_jurisdictions(query)
        .map(|(codes, _)| codes)
        .unwrap_or_default()
}

/// Comme [`query_jurisdictions`], mais renvoie aussi la requête **débarrassée
/// des tokens pays/gentilé** (tokens d'origine, casse/accents préservés) —
/// l'entrée de la jambe « pays nommé » du ranking (ADR 0238) : dans
/// « conditions du divorce au sénégal », le token « sénégal » désigne le
/// corpus, pas le contenu — les articles de fond du divorce sénégalais ne le
/// contiennent pas et le BM25 favorise les docs qui le portent (conventions
/// fiscales…). `None` si aucun pays nommé ; si rien ne subsiste après retrait
/// (requête réduite au pays), la requête d'origine est renvoyée telle quelle.
pub fn strip_query_jurisdictions(query: &str) -> Option<(Vec<&'static str>, String)> {
    let folded = fold(query);
    if folded.is_empty() {
        return None;
    }
    // Tokens d'origine alignés sur leur forme foldée (mêmes séparateurs).
    let orig_tokens: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let tokens: Vec<String> = orig_tokens.iter().map(|t| fold(t)).collect();
    // Un gentilé en composé bilatéral « franco-algérien » nomme un TRAITÉ
    // (accord franco-algérien, convention franco-sénégalaise — corpus INTL),
    // pas le droit interne du pays : le token précédé de « franco » ne
    // compte pas comme désignation de juridiction.
    let bilateral = |i: usize, tokens: &[String]| i > 0 && tokens[i - 1] == "franco";
    let mut matched = vec![false; tokens.len()];
    let mut codes: Vec<&'static str> = Vec::new();
    for (code, stems) in COUNTRIES {
        let mut hit = false;
        for s in *stems {
            match s.strip_suffix('*') {
                Some(prefix) => {
                    for i in 0..tokens.len() {
                        if tokens[i].starts_with(prefix) && !bilateral(i, &tokens) {
                            matched[i] = true;
                            hit = true;
                        }
                    }
                }
                None if s.contains(' ') => {
                    let parts: Vec<&str> = s.split(' ').collect();
                    for i in 0..tokens.len().saturating_sub(parts.len() - 1) {
                        if (0..parts.len()).all(|j| tokens[i + j] == parts[j]) {
                            for slot in matched.iter_mut().skip(i).take(parts.len()) {
                                *slot = true;
                            }
                            hit = true;
                        }
                    }
                }
                None => {
                    for i in 0..tokens.len() {
                        if tokens[i] == *s && !bilateral(i, &tokens) {
                            matched[i] = true;
                            hit = true;
                        }
                    }
                }
            }
        }
        if hit {
            codes.push(code);
        }
    }
    if codes.is_empty() {
        return None;
    }
    let stripped: Vec<&str> = orig_tokens
        .iter()
        .zip(&matched)
        .filter(|(_, m)| !**m)
        .map(|(t, _)| *t)
        .collect();
    let stripped = if stripped.is_empty() {
        query.to_string()
    } else {
        stripped.join(" ")
    };
    Some((codes, stripped))
}

#[cfg(test)]
mod tests {
    use super::query_jurisdictions;

    #[test]
    fn demonym_and_country_name_detected() {
        assert_eq!(
            query_jurisdictions("code de la famille sénégalais"),
            vec!["SN"]
        );
        assert_eq!(query_jurisdictions("code civil du sénégal"), vec!["SN"]);
        assert_eq!(
            query_jurisdictions("conditions du divorce au Sénégal"),
            vec!["SN"]
        );
        assert_eq!(query_jurisdictions("droit du travail belge"), vec!["BE"]);
    }

    #[test]
    fn domestic_query_detects_nothing() {
        assert!(query_jurisdictions("responsabilité du fait des choses").is_empty());
        assert!(query_jurisdictions("article L. 442-1 du code de commerce").is_empty());
        assert!(query_jurisdictions("").is_empty());
    }

    #[test]
    fn short_exact_forms_do_not_prefix_match() {
        // « mali » exact matche, « malice » non ; « niger » ≠ « nigeria ».
        assert_eq!(query_jurisdictions("code minier du mali"), vec!["ML"]);
        assert!(query_jurisdictions("malice et intention de nuire").is_empty());
        assert_eq!(query_jurisdictions("constitution du niger"), vec!["NE"]);
        assert_eq!(
            query_jurisdictions("droit des sociétés au nigeria"),
            vec!["NG"]
        );
    }

    #[test]
    fn congo_is_ambiguous() {
        assert_eq!(
            query_jurisdictions("code de la famille congolais"),
            vec!["CD", "CG"]
        );
    }

    #[test]
    fn strip_removes_country_tokens() {
        use super::strip_query_jurisdictions;
        assert_eq!(
            strip_query_jurisdictions("conditions du divorce au sénégal"),
            Some((vec!["SN"], "conditions du divorce au".to_string()))
        );
        assert_eq!(
            strip_query_jurisdictions("droit du travail belge"),
            Some((vec!["BE"], "droit du travail".to_string()))
        );
        // Sous-chaîne multi-mots : les deux tokens tombent.
        assert_eq!(
            strip_query_jurisdictions("successions aux pays bas"),
            Some((vec!["NL"], "successions aux".to_string()))
        );
        // Requête réduite au pays : renvoyée telle quelle.
        assert_eq!(
            strip_query_jurisdictions("sénégal"),
            Some((vec!["SN"], "sénégal".to_string()))
        );
        assert_eq!(
            strip_query_jurisdictions("trouble anormal de voisinage"),
            None
        );
    }

    #[test]
    fn bilateral_compound_is_not_a_country() {
        use super::strip_query_jurisdictions;
        // « franco-algérien » nomme un traité, pas le droit interne algérien.
        assert_eq!(
            strip_query_jurisdictions("accord franco-algérien certificat de résidence"),
            None
        );
        assert_eq!(
            strip_query_jurisdictions("convention franco-sénégalaise d'exequatur"),
            None
        );
        // Pays nommé nu À CÔTÉ d'un composé : le composé ne compte pas, le nom nu si.
        assert_eq!(
            strip_query_jurisdictions("accord franco-algérien et droit algérien"),
            Some((vec!["DZ"], "accord franco algérien et droit".to_string()))
        );
    }
}
