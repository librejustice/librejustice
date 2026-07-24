//! Récolte des n-grammes du vocabulaire d'autocomplétion (ADR 0216).

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::body_tok::is_stopword;

/// Mots-outils qui **cassent la proposition** : copules, auxiliaires, modaux,
/// relatifs, pronoms sujets, connecteurs — la plupart absents de la liste
/// tantivy `French` (intouchable, parité BM25). Un n-gramme qui en contient
/// un chevauche deux propositions (« garde à vue *est* immédiatement »,
/// « garde à vue *il* résulte ») : interdits partout, bords comme intérieur.
static CLAUSE_BREAKERS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "ainsi",
        "alors",
        "aussi",
        "comme",
        "deja",
        "doit",
        "doivent",
        "donc",
        "dont",
        "elle",
        "elles",
        "encore",
        "enfin",
        "ensuite",
        "est",
        "ete",
        "etre",
        "il",
        "ils",
        "lorsqu",
        "lorsque",
        "notamment",
        "on",
        "peut",
        "peuvent",
        "puis",
        "puisque",
        "quand",
        "seulement",
        "si",
        "sinon",
        "toutefois",
    ]
    .into_iter()
    .collect()
});

/// Prépositions et déterminants propres au suggest qui ne peuvent **ni ouvrir
/// ni fermer** un n-gramme mais restent admis à l'intérieur, comme les
/// stopwords de l'index (« placement *sous* contrôle judiciaire »).
static BORDER_ONLY: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "apres", "autre", "autres", "avant", "celle", "celles", "celui", "ceux", "chaque", "entre",
        "moins", "outre", "plus", "selon", "sous", "tous", "tout", "toute", "toutes", "tres",
        "vers",
    ]
    .into_iter()
    .collect()
});

/// `true` si `folded` ne peut pas être un bord de n-gramme (stopword de
/// l'index, préposition de bordure ou casseur de proposition).
fn is_border_word(folded: &str) -> bool {
    is_stopword(folded) || BORDER_ONLY.contains(folded) || CLAUSE_BREAKERS.contains(folded)
}

/// N max des n-grammes récoltés (1 à 5 mots — « code de procédure civile »
/// tient en 4, au-delà de 5 ce qui reste fréquent est du boilerplate).
pub const MAX_N: usize = 5;

/// Tokens max d'une clé du vocabulaire, toutes provenances : les titres
/// `legal_text` injectés entiers dépassent [`MAX_N`]. Borne le filtre
/// d'injection au build ET la profondeur de contexte du probe (qui doit
/// pouvoir re-matcher un titre entier).
pub const MAX_KEY_TOKENS: usize = 8;

/// Séparateur clé pliée ↔ forme d'affichage dans les clés du FST
/// (`folded\x00display`, le suffixe omis quand les deux coïncident). 0x00 est
/// inférieur à tout octet de token : le tri et le match par préfixe plié
/// restent corrects.
pub const DISPLAY_SEP: u8 = 0x00;

/// Émet les spans `[start, end)` des n-grammes 1..=[`MAX_N`] d'une suite de
/// tokens body foldés. Trois règles : un n-gramme ne **commence ni ne
/// finit** par un mot de bordure (stopword de l'index ou [`BORDER_ONLY`],
/// admis à l'intérieur — « tribunal de commerce », « placement sous contrôle
/// judiciaire ») ; il ne **contient jamais** de casseur de proposition
/// ([`CLAUSE_BREAKERS`]) ; il ne **tronque jamais une suite de tokens
/// numériques** — les montants éclatés par le tokenizer (« 1 500 »)
/// produiraient sinon des fragments (« somme de 1 ») dont le df agrège des
/// milliers de montants distincts et écrase le ranking. Les chiffres passent
/// partout ailleurs (« article 700 », « loi du 29 juillet ») : les numéros
/// rares — RG, dates isolées — sont éliminés par le plancher de df, pas par
/// une règle. L'appelant joint les tokens du span (formes pliée et affichée,
/// alignées index à index par le tokenizer).
pub fn harvest_ngrams(toks: &[String], mut emit: impl FnMut(usize, usize)) {
    let border: Vec<bool> = toks.iter().map(|t| is_border_word(t)).collect();
    let broken: Vec<bool> = toks
        .iter()
        .map(|t| CLAUSE_BREAKERS.contains(t.as_str()))
        .collect();
    let digit: Vec<bool> = toks
        .iter()
        .map(|t| t.chars().any(|c| c.is_ascii_digit()))
        .collect();
    let cuts_number = |s: usize, e: usize| {
        (digit[s] && s > 0 && digit[s - 1]) || (digit[e - 1] && e < toks.len() && digit[e])
    };
    for start in 0..toks.len() {
        if border[start] {
            continue;
        }
        for end in start + 1..=(start + MAX_N).min(toks.len()) {
            // Casseur atteint : tout span plus long de ce départ le contient.
            if broken[end - 1] {
                break;
            }
            if !border[end - 1] && !cuts_number(start, end) {
                emit(start, end);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body_tok::tokenize;

    fn all(query: &str) -> Vec<String> {
        let toks = tokenize(query);
        let mut out = Vec::new();
        harvest_ngrams(&toks, |start, end| out.push(toks[start..end].join(" ")));
        out
    }

    #[test]
    fn collocation_a_stopword_interne_capturee() {
        let ngrams = all("tribunal de commerce de paris");
        // La collocation à stopword interne émerge en trigramme…
        assert!(ngrams.contains(&"tribunal de commerce".to_string()));
        assert!(ngrams.contains(&"commerce de paris".to_string()));
        // …mais aucun n-gramme ne commence ni ne finit par un stopword.
        assert!(!ngrams.iter().any(|n| n.starts_with("de ")));
        assert!(!ngrams.iter().any(|n| n.ends_with(" de")));
        assert!(!ngrams.contains(&"de".to_string()));
        // Unigrammes de contenu présents.
        assert!(ngrams.contains(&"tribunal".to_string()));
        assert!(ngrams.contains(&"paris".to_string()));
    }

    #[test]
    fn bigramme_adjacent_sans_stopword() {
        let ngrams = all("licenciement economique collectif");
        assert!(ngrams.contains(&"licenciement economique".to_string()));
        assert!(ngrams.contains(&"licenciement economique collectif".to_string()));
        assert!(ngrams.contains(&"economique collectif".to_string()));
    }

    #[test]
    fn references_et_dates_numerotees_admises() {
        // Les références numérotées sont du vocabulaire de premier plan…
        let ngrams = all("article 700 du code");
        assert!(ngrams.contains(&"article 700".to_string()));
        // …y compris les lois citées par leur date.
        let ngrams = all("loi du 29 juillet 1881 presse");
        assert!(ngrams.contains(&"loi du 29".to_string()));
        assert!(ngrams.contains(&"29 juillet 1881".to_string()));
        assert!(ngrams.contains(&"juillet 1881".to_string()));
    }

    #[test]
    fn suite_numerique_jamais_tronquee() {
        // « 1 500 » (montant éclaté par le tokenizer) ne s'échantillonne pas :
        // le fragment agrégerait le df de milliers de montants distincts.
        let ngrams = all("somme de 1 500 euros");
        assert!(!ngrams.contains(&"somme de 1".to_string()));
        assert!(!ngrams.contains(&"1".to_string()));
        assert!(!ngrams.contains(&"500 euros".to_string()));
        // La suite entière, elle, passe (df individuel faible → plancher).
        assert!(ngrams.contains(&"1 500 euros".to_string()));
        assert!(ngrams.contains(&"somme".to_string()));
        // Même règle sur les identifiants multi-tokens : pas de préfixe coupé.
        let ngrams = all("loi 78 17 informatique");
        assert!(!ngrams.contains(&"loi 78".to_string()));
        assert!(ngrams.contains(&"loi 78 17".to_string()));
    }

    #[test]
    fn quadri_et_pentagrammes_recoltes() {
        let ngrams = all("code de procedure civile applicable");
        assert!(ngrams.contains(&"code de procedure civile".to_string()));
        assert!(ngrams.contains(&"code de procedure civile applicable".to_string()));
    }

    #[test]
    fn casseurs_de_proposition_bloques_partout() {
        // « est » (absent de la liste tantivy) casse la proposition : aucun
        // n-gramme ne le contient — ni « garde à vue est », ni « garde à vue
        // est immédiatement ».
        let ngrams = all("la garde a vue est immediatement notifiee");
        assert!(!ngrams.iter().any(|n| n.contains("est")));
        assert!(ngrams.contains(&"garde a vue".to_string()));
        assert!(ngrams.contains(&"immediatement notifiee".to_string()));
        // Idem pour le relatif « dont », même à l'intérieur.
        let ngrams = all("servitude dont beneficie le fonds");
        assert!(!ngrams.iter().any(|n| n.contains("dont")));
        assert!(ngrams.contains(&"servitude".to_string()));
    }

    #[test]
    fn prepositions_de_bordure_admises_a_l_interieur() {
        let ngrams = all("placement sous controle judiciaire");
        assert!(ngrams.contains(&"placement sous controle judiciaire".to_string()));
        assert!(!ngrams
            .iter()
            .any(|n| n.starts_with("sous ") || n.ends_with(" sous")));
        assert!(!ngrams.contains(&"sous".to_string()));
    }

    #[test]
    fn spans_alignes_sur_la_forme_affichee() {
        // Les spans se rejouent sur les tokens display (mêmes frontières) :
        // la forme accentuée du n-gramme s'obtient du même [start, end).
        let folded = tokenize("congés payés acquis");
        let display = crate::body_tok::tokenize_lower("congés payés acquis");
        let mut pairs = Vec::new();
        harvest_ngrams(&folded, |s, e| {
            pairs.push((folded[s..e].join(" "), display[s..e].join(" ")));
        });
        assert!(pairs.contains(&("conges payes".to_string(), "congés payés".to_string())));
    }
}
