//! Le nom d'usage cité dans un corps : « la jurisprudence Danthony » vise une
//! décision qu'aucun numéro ne nomme, donc qu'aucun lexer ne voit.

use std::collections::HashMap;

use lj_core::address::CaseAddress;
use lj_core::case_names::{case_name_key as key, NameIndex, MAX_NAME_WORDS, NAME_BEARERS_MAX};
use lj_core::citation::name_words;

/// A named decision, as the snapshot is built from it.
#[derive(Debug, Clone)]
pub struct NamedCase {
    pub keys: Vec<String>,
    /// `None` when the decision lacks the court or the docket an address needs.
    pub address: Option<CaseAddress>,
    /// Spans and renvois aiming at it, the count the search card ranks a name's bearers by.
    pub citations: u32,
}

/// The names a body can cite: the exact key a single decision carries, and the words of every
/// name, read as a query reads them.
#[derive(Debug, Default, Clone)]
pub struct CaseNameSnapshot {
    par_cle: HashMap<String, CaseAddress>,
    /// First folded word to the length of the longest key it starts, so one very
    /// long key no longer sets the bound of all the others.
    par_tete: HashMap<String, usize>,
    names: NameIndex,
    /// Address and citations of each named decision, by its rank in `names`.
    named: Vec<(Option<CaseAddress>, u32)>,
}

/// What the deciding court brings to the sorting of candidate targets.
#[derive(Debug, Default, Clone, Copy)]
pub struct Citing<'a> {
    /// `cjue` or `cedh`, the European court that decides. A European court naming a French
    /// target almost always names a homonym, so the target is refused by default.
    pub european: Option<&'static str>,
    /// Reading date, ISO. A decision does not cite what comes after it.
    pub date: Option<&'a str>,
}

impl CaseNameSnapshot {
    pub fn new(named: impl IntoIterator<Item = NamedCase>) -> Self {
        let named: Vec<NamedCase> = named.into_iter().collect();
        let mut porteurs: HashMap<&str, usize> = HashMap::new();
        for cle in named.iter().flat_map(|n| &n.keys) {
            *porteurs.entry(cle).or_default() += 1;
        }
        let mut par_cle = HashMap::new();
        let mut par_tete: HashMap<String, usize> = HashMap::new();
        for n in &named {
            let Some(address) = &n.address else { continue };
            for cle in &n.keys {
                if cle.is_empty() || porteurs[cle.as_str()] > 1 {
                    continue;
                }
                let mut mots = cle.split(' ');
                let Some(tete) = mots.next() else { continue };
                let n = mots.count() + 1;
                let borne = par_tete.entry(tete.to_string()).or_default();
                *borne = (*borne).max(n);
                par_cle.insert(cle.clone(), address.clone());
            }
        }
        let names = NameIndex::new(named.iter().map(|n| n.keys.clone()));
        let named = named
            .into_iter()
            .map(|n| (n.address, n.citations))
            .collect();
        CaseNameSnapshot {
            par_cle,
            par_tete,
            names,
            named,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.named.is_empty()
    }
}

/// The two courts of the address that are not French.
const COURS_EUROPEENNES: [&str; 2] = ["cjue", "cedh"];

/// Codepoints of the identity block a European decision opens with (`AFFAIRE
/// LA ROSA ET 3 AUTRES c. ITALIE (No 5)`, `En l'affaire Granata c. France`).
/// It names the present case, and the catalogue answers with a sibling of the
/// same family.
const EUROPEAN_HEADER: usize = 400;

/// What the name key alone cannot see: a European judge's `arrêt Kadi` taken for a
/// homonymous French decision, a decision handed down after the citing one.
fn impossible(cle: &str, cible: &CaseAddress, citing: &Citing<'_>, banniere: Option<&str>) -> bool {
    let cour = cible.court.as_deref().unwrap_or_default();
    if citing.european.is_some()
        && !COURS_EUROPEENNES.contains(&cour)
        && !nomme_le_droit_francais(cle, banniere)
    {
        return true;
    }
    match (cible.date.as_deref(), citing.date) {
        (Some(cible), Some(citante)) => cible > citante,
        _ => false,
    }
}

/// A European decision whose respondent state is France reviews French law and
/// names it (`décision Czabaj`, Darmont, Gardedieu, Nicolo). Unless the name is
/// the applicant's own, which the
/// banner also carries: `SANCHEZ c. FRANCE` writing `Sanchez` names itself.
fn nomme_le_droit_francais(cle: &str, banniere: Option<&str>) -> bool {
    let Some(b) = banniere else {
        return false;
    };
    b.contains(" c france") && !cle.split(' ').any(|mot| b.split(' ').any(|w| w == mot))
}

/// Sur 34 déclenchements relus, la locution fait la citation et le nom nu tombe
/// sur les parties ou l'en-tête. Accents et pluriel sont pliés par [`egal_plie`].
const LOCUTIONS: [&str; 4] = ["jurisprudence", "arret", "decision", "affaire"];

/// Mots qui s'intercalent entre la locution et le nom sans rien changer.
const LIAISONS: [&str; 2] = ["dite", "dit"];

/// Punctuation between the locution and the name: the greffe banner
/// (`AFFAIRE : Marie-Claude X…`), the apposed party (`l'agent d'affaires,
/// Beaufils`), the sentence already closed (`de cette décision. Gaston B…`).
fn severed(separator: &str) -> bool {
    separator.contains(['.', ';', ':', '!', '?', ',', '\n'])
}

/// `chiffre d'affaires croissant`, `chargé d'affaires Marié`: glued to `d'`,
/// `affaire` is the common noun and announces nothing.
fn common_noun(mots: &[Mot<'_>], rang: usize) -> bool {
    egal_plie(mots[rang].mot, "affaire", true)
        && mots[rang].avant.starts_with(['\'', '’'])
        && rang > 0
        && egal_plie(mots[rang - 1].mot, "d", false)
}

/// Spans `(début, fin, clé pendante)` des noms d'usage cités, en **codepoints**,
/// triés et sans chevauchement. `soi` = les clés pendantes de la décision qui
/// porte le texte : elle se nomme dans son propre corps sans se citer.
pub fn cited_case_name_spans(
    text: &str,
    snap: &CaseNameSnapshot,
    soi: &[CaseAddress],
    citing: Citing<'_>,
) -> Vec<(usize, usize, CaseAddress)> {
    if snap.is_empty() {
        return Vec::new();
    }
    let mots = mots(text);
    // The identity banner, folded once: it carries the respondent state and the
    // applicant's name, which nothing else brings to the extraction.
    let banniere = citing
        .european
        .is_some()
        .then(|| key(&text.chars().take(EUROPEAN_HEADER).collect::<String>()));
    let mut spans: Vec<(usize, usize, CaseAddress)> = Vec::new();
    let mut cle = String::new();
    let mut coupes: Vec<usize> = Vec::new();
    for (rang, mot) in mots.iter().enumerate() {
        if !LOCUTIONS.iter().any(|l| egal_plie(mot.mot, l, true)) {
            continue;
        }
        if common_noun(&mots, rang) {
            continue;
        }
        let mut tete = rang + 1;
        if mots
            .get(tete)
            .is_some_and(|m| LIAISONS.iter().any(|l| egal_plie(m.mot, l, false)))
        {
            tete += 1;
        }
        if tete >= mots.len() || mots[rang + 1..=tete].iter().any(|m| severed(m.avant)) {
            continue;
        }
        let par_mots = !egal_plie(mot.mot, "affaires", false)
            && mot.mot.chars().any(char::is_lowercase)
            && !(rang > 0
                && DETERMINANTS
                    .iter()
                    .any(|d| egal_plie(mots[rang - 1].mot, d, false)));
        let lu = par_la_cle(&mots, tete, snap, &mut cle, &mut coupes).or_else(|| {
            par_mots
                .then(|| par_les_mots(&mots, tete, snap, soi, &citing, banniere.as_deref()))
                .flatten()
        });
        let Some((fin, cle_lue, nommee)) = lu else {
            continue;
        };
        let debut = mots[tete].debut;
        let refuse = (citing.european.is_some() && debut < EUROPEAN_HEADER)
            || soi.contains(&nommee)
            || impossible(&cle_lue, &nommee, &citing, banniere.as_deref())
            || nom_tronque(&mots, fin + 1)
            || spans.last().is_some_and(|(_, f, _)| *f > debut);
        if !refuse {
            spans.push((debut, mots[fin].fin, nommee));
        }
    }
    spans
}

/// The longest exact key the words write from `tete`, with its last word.
fn par_la_cle(
    mots: &[Mot<'_>],
    tete: usize,
    snap: &CaseNameSnapshot,
    cle: &mut String,
    coupes: &mut Vec<usize>,
) -> Option<(usize, String, CaseAddress)> {
    let mot_tete = key(mots[tete].mot);
    let &borne = snap.par_tete.get(&mot_tete)?;
    let lim = borne.min(mots.len() - tete);
    // The keys of one head are prefixes of one another: extend once up to `lim`, then read the
    // cuts back from the longest to the shortest.
    cle.clear();
    coupes.clear();
    cle.push_str(&mot_tete);
    coupes.push(cle.len());
    for i in 1..lim {
        cle.push(' ');
        cle.push_str(&key(mots[tete + i].mot));
        coupes.push(cle.len());
    }
    (1..=lim).rev().find_map(|taille| {
        let lue = &cle[..coupes[taille - 1]];
        snap.par_cle
            .get(lue)
            .map(|nommee| (tete + taille - 1, lue.to_string(), nommee.clone()))
    })
}

/// Words before a locution that make it a common noun: « cette décision Mme B
/// soutient », « dans une affaire CAS n° 24-082023 ».
const DETERMINANTS: [&str; 6] = ["un", "une", "ce", "cet", "cette", "ces"];

const CIVILITES: [&str; 6] = ["m", "mme", "mlle", "mr", "monsieur", "madame"];

/// What may join two words of one name: `Rinner-Kühn`, `U/Parlement`, `Apple & Pear`. A glued
/// `&` opens an HTML entity (`K&uuml;hne`).
fn joins_a_name(separator: &str) -> bool {
    separator
        .chars()
        .all(|c| c.is_whitespace() || matches!(c, '-' | '\'' | '’' | '/'))
        || (separator.trim() == "&" && separator.starts_with(char::is_whitespace))
}

/// The name the words from `tete` write, read as a query reads it: its distinctive words,
/// up to the first that no name shares with the others, in the names of
/// [`NAME_BEARERS_MAX`] decisions at most. « jurisprudence Tropic » cites Société Tropic
/// Travaux Signalisation. A day written after the name keeps the bearers of that day.
fn par_les_mots(
    mots: &[Mot<'_>],
    tete: usize,
    snap: &CaseNameSnapshot,
    soi: &[CaseAddress],
    citing: &Citing<'_>,
    banniere: Option<&str>,
) -> Option<(usize, String, CaseAddress)> {
    if !mots[tete]
        .mot
        .chars()
        .next()
        .is_some_and(char::is_uppercase)
        || !mots[tete]
            .avant
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, '«' | '"' | '“'))
    {
        return None;
    }
    let mut lus: Vec<String> = Vec::new();
    let mut fin = None;
    for (i, m) in mots
        .iter()
        .enumerate()
        .take(tete + MAX_NAME_WORDS)
        .skip(tete)
    {
        if i > tete && (severed(m.avant) || !joins_a_name(m.avant)) {
            break;
        }
        let mot = key(m.mot);
        if name_words(&mot).is_empty() {
            match i == tete {
                true => return None,
                false => continue,
            }
        }
        if m.mot.starts_with(char::is_lowercase) && !suit_dans_une_cle(snap, &lus, &mot) {
            break;
        }
        let initiale = mot.chars().count() < 2;
        lus.push(mot);
        let words: Vec<&str> = lus.iter().map(String::as_str).collect();
        if snap.names.holding(&words).is_empty() {
            lus.pop();
            break;
        }
        if !initiale {
            fin = Some(i);
        }
    }
    let fin = fin?;
    if !lus
        .iter()
        .any(|w| w.chars().count() >= 3 && !CIVILITES.contains(&w.as_str()))
        || mots
            .get(fin + 1)
            .is_some_and(|m| m.avant.trim_start().starts_with(':'))
    {
        return None;
    }
    let words: Vec<&str> = lus.iter().map(String::as_str).collect();
    let cle = words.join(" ");
    let mut candidates: Vec<(Option<&CaseAddress>, u32)> = snap
        .names
        .bearers(&words, NAME_BEARERS_MAX)
        .into_iter()
        .filter(|&rank| {
            snap.names
                .keys(rank)
                .iter()
                .any(|k| writes_in_order(k, &words))
        })
        .map(|rank| {
            let (address, citations) = &snap.named[rank];
            (address.as_ref(), *citations)
        })
        .filter(|(address, _)| address.is_none_or(|a| !impossible(&cle, a, citing, banniere)))
        .collect();
    let suite: String = mots[fin + 1..]
        .iter()
        .take(6)
        .map(|m| format!("{}{}", m.avant, m.mot))
        .collect();
    if let Some(jour) = lj_core::citation::query_day(&suite) {
        candidates.retain(|(address, _)| {
            address.is_some_and(|a| a.date.as_deref() == Some(jour.as_str()))
        });
    }
    // « Legros et autres » under the ECHR's pen is Legros et autres c. France, not the CJUE's
    // Léopold Legros et autres, cited twice as often.
    let siennes = |address: &Option<&CaseAddress>| {
        address.is_some_and(|a| a.court.is_some() && a.court.as_deref() == citing.european)
    };
    if candidates.iter().any(|(address, _)| siennes(address)) {
        candidates.retain(|(address, _)| siennes(address));
    }
    if candidates
        .iter()
        .any(|(address, _)| address.is_some_and(|a| soi.contains(a)))
    {
        return None;
    }
    let nommee = choisie(&candidates)?;
    let cour = nommee.court.as_deref().unwrap_or_default();
    if citing.european.is_none()
        && COURS_EUROPEENNES.contains(&cour)
        && !names_the_court(mots, cour)
    {
        return None;
    }
    Some((fin, cle, nommee.clone()))
}

/// Whether a French body names the European court somewhere. The CELLAR titles carry given
/// names, which a locution writes alone (« l'arrêt Isabelle »): requiring the court drops those.
fn names_the_court(mots: &[Mot<'_>], cour: &str) -> bool {
    let marques: &[&[&str]] = match cour {
        "cjue" => &[
            &["cjue"],
            &["cjce"],
            &["cour", "de", "justice"],
            &["tpi"],
            &["tpice"],
            &["tribunal", "de", "l", "union"],
            &[
                "tribunal",
                "de",
                "premiere",
                "instance",
                "des",
                "communautes",
            ],
        ],
        _ => &[
            &["cedh"],
            &["cour", "edh"],
            &["cour", "europeenne"],
            &["strasbourg"],
        ],
    };
    let plies: Vec<String> = mots.iter().map(|m| key(m.mot)).collect();
    marques.iter().any(|marque| {
        plies
            .windows(marque.len())
            .any(|lus| lus.iter().zip(*marque).all(|(l, m)| l == m))
    })
}

/// Whether a key writes `mot` right after the last word read, citation words aside: a word in
/// lower case goes on with « Van der Hulst » and « Legros et autres », not with « Canal pour
/// soutenir », whose « pour » is in « Groupe Canal contre Office de l'Union européenne pour… ».
fn suit_dans_une_cle(snap: &CaseNameSnapshot, lus: &[String], mot: &str) -> bool {
    let Some(precedent) = lus.last() else {
        return false;
    };
    let words: Vec<&str> = lus.iter().map(String::as_str).chain([mot]).collect();
    snap.names.holding(&words).into_iter().any(|rank| {
        snap.names.keys(rank).iter().any(|cle| {
            name_words(cle)
                .windows(2)
                .any(|p| p[0] == precedent && p[1] == mot)
        })
    })
}

/// The key holds the words in the order the body writes them: « Commission/Grèce » is not
/// « Dimos Ano Liosion (Grèce) contre Commission ».
fn writes_in_order(key: &str, words: &[&str]) -> bool {
    let mut suite = key.split(' ');
    words.iter().all(|w| suite.any(|k| k == *w))
}

/// The most cited of a name's bearers, when it gathers twice the citations of the next: a
/// European name has its judgment, its opinion and its orders, and the judgment is the one
/// cited.
fn choisie<'a>(candidates: &[(Option<&'a CaseAddress>, u32)]) -> Option<&'a CaseAddress> {
    let mut tri = candidates.to_vec();
    tri.sort_by_key(|&(_, citations)| std::cmp::Reverse(citations));
    let (nommee, premiere) = *tri.first()?;
    match tri.get(1) {
        Some(&(_, seconde)) if premiere < 2 * seconde.max(1) => None,
        _ => nommee,
    }
}

/// Le nom reconnu n'est que la tête d'un nom plus long : « arrêt Soulier et
/// Doke » vise la décision de la CJUE, pas l'arrêt Soulier du Conseil d'État.
///
/// Three more continuations: a capitalised word right after
/// (`arrêt Kadi II`, `López Brea et Hidalgo Palacios` read as `López`), a mark
/// glued to the name (`Weber's Wine World`, `Marie-Claire Tatete`), a slash or
/// an ampersand (`France Télécom/Commission`, `Benjamin & Wilson`). The mark
/// must be glued: `jurisprudence Ternon '` and `arrêt Ringeisen - sur` close a
/// sentence, they do not lengthen a name.
fn nom_tronque(mots: &[Mot<'_>], apres: usize) -> bool {
    let Some(suivant) = mots.get(apres) else {
        return false;
    };
    if egal_plie(suivant.mot, "et", false) {
        return mots
            .get(apres + 1)
            .is_some_and(|m| m.mot.chars().next().is_some_and(char::is_uppercase));
    }
    let sep = suivant.avant;
    sep.starts_with(['-', '\'', '’'])
        || sep.trim_matches([' ', '\t']).starts_with(['&', '/'])
        || (!sep.is_empty()
            && sep.chars().all(|c| c == ' ' || c == '\t')
            && suivant.mot.chars().next().is_some_and(char::is_uppercase))
}

struct Mot<'a> {
    mot: &'a str,
    debut: usize,
    fin: usize,
    /// Text separating this word from the previous one: it carries the
    /// punctuation that splitting into words would otherwise throw away.
    avant: &'a str,
}

/// Découpe en mots avec leurs offsets en codepoints, comme les spans stockés.
fn mots(text: &str) -> Vec<Mot<'_>> {
    let mut mots = Vec::new();
    let mut ouvert: Option<(usize, usize)> = None;
    let mut precedent = 0usize;
    for (rang, (octet, c)) in text.char_indices().enumerate() {
        if c.is_alphanumeric() {
            if ouvert.is_none() {
                ouvert = Some((rang, octet));
            }
        } else if let Some((r, o)) = ouvert.take() {
            mots.push(Mot {
                mot: &text[o..octet],
                debut: r,
                fin: rang,
                avant: &text[precedent..o],
            });
            precedent = octet;
        }
    }
    if let Some((r, o)) = ouvert {
        mots.push(Mot {
            mot: &text[o..],
            debut: r,
            fin: r + text[o..].chars().count(),
            avant: &text[precedent..o],
        });
    }
    mots
}

/// Égalité accents et casse repliés, sans allocation. `pluriel` accepte le `s`
/// final du mot lu : « arrêts Danthony » cite comme « arrêt Danthony ».
fn egal_plie(mot: &str, cible: &str, pluriel: bool) -> bool {
    let mut lu = mot.chars().flat_map(lj_fold::fold_char);
    let mut attendu = cible.chars();
    loop {
        match (lu.next(), attendu.next()) {
            (Some(a), Some(b)) if a == b => continue,
            (None, None) => return true,
            (Some('s'), None) if pluriel => return lu.next().is_none(),
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nommee(keys: &[&str], court: &str, docket: &str, date: &str) -> NamedCase {
        NamedCase {
            keys: keys.iter().map(|k| k.to_string()).collect(),
            address: Some(CaseAddress::new(court, docket, date)),
            citations: 0,
        }
    }

    fn snap() -> CaseNameSnapshot {
        CaseNameSnapshot::new([
            nommee(&["danthony"], "ce", "335033", "2011-12-23"),
            nommee(&["soulier"], "ce", "223041", "2002-10-30"),
            nommee(
                &[
                    "societe tropic travaux signalisation",
                    "tropic travaux signalisation",
                ],
                "ce",
                "291545",
                "2007-07-16",
            ),
            nommee(&["granata c france"], "cedh", "39626/98", "2003-03-27"),
            nommee(&["czabaj"], "ce", "389011", "2016-07-13"),
            nommee(&["sanchez"], "ce", "401800", "2017-05-10"),
        ])
    }

    /// A French 2024 citing decision, later than every target in the snapshot.
    fn citante() -> Citing<'static> {
        Citing {
            european: None,
            date: Some("2024-01-01"),
        }
    }

    /// `court|docket` of each span, an address in its readable test form.
    fn rendu(a: &CaseAddress) -> String {
        let c = |o: &Option<String>| o.clone().unwrap_or_default();
        format!("{}|{}", c(&a.court), c(&a.docket))
    }

    fn cles(text: &str, soi: &[CaseAddress]) -> Vec<String> {
        cited_case_name_spans(text, &snap(), soi, citante())
            .into_iter()
            .map(|(_, _, r)| rendu(&r))
            .collect()
    }

    #[test]
    fn la_locution_fait_la_citation() {
        for texte in [
            "au sens de la jurisprudence Danthony et ont été de nature",
            "le Conseil d'Etat applique la jurisprudence dite Danthony à tout vice",
            "il résulte de l'arrêt Danthony que le vice",
            "la décision Danthony du 23 décembre 2011",
            "les arrêts Danthony et suivants",
            "il devra faire application de la « jurisprudence Danthony » dans la mesure où",
        ] {
            assert_eq!(cles(texte, &[]), vec!["ce|335033"], "texte {texte:?}");
        }
    }

    #[test]
    fn le_nom_nu_ne_cite_pas() {
        assert!(cles("M. Danthony demande l'annulation du décret", &[]).is_empty());
    }

    /// « Soulier et Doke » names a CJEU judgment, never the Conseil d'État's « Soulier ».
    #[test]
    fn le_nom_tronque_ne_cite_pas() {
        assert!(cles(
            "la solution de l'arrêt Soulier et Doke n'est pas transposable",
            &[]
        )
        .is_empty());
        assert_eq!(
            cles(
                "compte tenu de l'arrêt Soulier qui ne permet le retrait",
                &[]
            ),
            vec!["ce|223041"]
        );
    }

    #[test]
    fn la_decision_ne_se_cite_pas_elle_meme() {
        assert!(cles(
            "la jurisprudence Danthony",
            &[CaseAddress::new("ce", "335033", "2011-12-23")]
        )
        .is_empty());
    }

    /// La bannière du greffe, la partie apposée et la phrase déjà close.
    #[test]
    fn la_ponctuation_entre_la_locution_et_le_nom_coupe() {
        for texte in [
            "N° Minute : 24/01897  AFFAIRE :  Danthony et associés  C/  [V]",
            "l'agent d'affaires, Danthony, était chargé de rédiger les statuts",
            "il n'y a pas lieu de revenir sur cette décision. Danthony répondait le 31 juillet",
            "R.G. 11/04606  Affaire\nDanthony X... épouse Y...",
        ] {
            assert!(cles(texte, &[]).is_empty(), "texte {texte:?}");
        }
    }

    #[test]
    fn le_chiffre_d_affaires_n_est_pas_une_locution() {
        assert!(cles("un chiffre d'affaires Danthony de 688 000 euros", &[]).is_empty());
        assert!(cles("le chargé d’affaires Danthony a signé", &[]).is_empty());
        assert_eq!(
            cles("la solution retenue dans l'affaire Danthony vaut ici", &[]),
            vec!["ce|335033"]
        );
    }

    /// A name continued by a numeral, a hyphen, a possessive, a slash or an
    /// ampersand names another case.
    #[test]
    fn le_nom_prolonge_ne_cite_pas() {
        for texte in [
            "il ressort de l'arrêt Danthony II, point 124, que le vice",
            "l'arrêt Danthony-Burckhardt du 22 octobre 1975",
            "l'arrêt Danthony's Wine World, C 147/01",
            "voir en ce sens l'arrêt Danthony/Commission, précité",
            "l'arrêt Danthony & Wilson c. Royaume-Uni",
        ] {
            assert!(cles(texte, &[]).is_empty(), "texte {texte:?}");
        }
    }

    /// A detached mark closes the sentence, it does not extend the name.
    #[test]
    fn la_marque_detachee_laisse_citer() {
        for texte in [
            "la jurisprudence Danthony ' 2°) Notamment les dispositions",
            "l'arrêt Danthony - sur l'organisation administrative",
        ] {
            assert_eq!(cles(texte, &[]), vec!["ce|335033"], "texte {texte:?}");
        }
    }

    /// `arrêt Kadi` under a European judge's pen names a European judgment,
    /// never the Conseil d'État decision carrying the same party name.
    #[test]
    fn la_juridiction_europeenne_ne_nomme_pas_une_cible_francaise() {
        let europeenne = Citing {
            european: Some("cedh"),
            date: Some("2024-01-01"),
        };
        assert!(cited_case_name_spans(
            "il ressort de l'arrêt Danthony que le vice",
            &snap(),
            &[],
            europeenne
        )
        .is_empty());
    }

    /// The ECHR ruling against France names French case law, and an applicant
    /// naming themselves is not citing anyone.
    #[test]
    fn l_etat_defendeur_rouvre_la_cible_francaise() {
        let bandeau = "CINQUIÈME SECTION AFFAIRE Sanchez c. FRANCE (Requête no 45581/15) ARRÊT ";
        let europeenne = Citing {
            european: Some("cedh"),
            date: Some("2024-01-01"),
        };
        let corps = format!(
            "{bandeau}{}Il résulte de la jurisprudence Czabaj que le délai court.",
            " ".repeat(400)
        );
        assert_eq!(
            cited_case_name_spans(&corps, &snap(), &[], europeenne)
                .into_iter()
                .map(|(_, _, r)| rendu(&r))
                .collect::<Vec<_>>(),
            vec!["ce|389011"]
        );
        // Same banner, but the name aimed at is the applicant's own.
        let soi = format!(
            "{bandeau}{}La requête de l'arrêt Sanchez est recevable.",
            " ".repeat(400)
        );
        assert!(cited_case_name_spans(&soi, &snap(), &[], europeenne).is_empty());
        // With no France in the banner, the French target stays refused.
        let turquie = format!(
            "QUATRIÈME SECTION AFFAIRE Yilmaz c. TURQUIE{}Il résulte de la jurisprudence Czabaj que.",
            " ".repeat(400)
        );
        assert!(cited_case_name_spans(&turquie, &snap(), &[], europeenne).is_empty());
    }

    /// A European decision's identity banner names the case at hand, and the
    /// catalogue answers with a sibling of the same family.
    #[test]
    fn le_bandeau_europeen_ne_cite_pas() {
        let europeenne = Citing {
            european: Some("cedh"),
            date: Some("2024-01-01"),
        };
        let entete = "PREMIÈRE SECTION AFFAIRE Granata c. France (no 3) (Requête no 39634/98)";
        assert!(cited_case_name_spans(entete, &snap(), &[], europeenne).is_empty());
        let corps = format!(
            "{}Il ressort de l'arrêt Granata c. France que",
            " ".repeat(420)
        );
        assert_eq!(
            cited_case_name_spans(&corps, &snap(), &[], europeenne)
                .into_iter()
                .map(|(_, _, r)| rendu(&r))
                .collect::<Vec<_>>(),
            vec!["cedh|39626/98"]
        );
    }

    #[test]
    fn une_decision_ne_cite_pas_ce_qui_la_suit() {
        let avant = Citing {
            european: None,
            date: Some("2005-06-01"),
        };
        assert!(cited_case_name_spans(
            "au sens de la jurisprudence Danthony et ont été de nature",
            &snap(),
            &[],
            avant
        )
        .is_empty());
    }

    #[test]
    fn le_nom_se_lit_a_ses_mots() {
        assert_eq!(
            cles("au regard de la jurisprudence Tropic, le recours", &[]),
            ["ce|291545"]
        );
        assert!(cles("la décision attaquée", &[]).is_empty());
    }

    fn porteuse(key: &str, court: &str, docket: &str, date: &str, citations: u32) -> NamedCase {
        NamedCase {
            citations,
            ..nommee(&[key], court, docket, date)
        }
    }

    fn citee(snap: &CaseNameSnapshot, text: &str) -> Vec<CaseAddress> {
        cited_case_name_spans(text, snap, &[], citante())
            .into_iter()
            .map(|(_, _, a)| a)
            .collect()
    }

    #[test]
    fn la_plus_citee_des_porteuses_quand_elle_domine() {
        let snap = CaseNameSnapshot::new([
            porteuse("angelidaki e a", "cjue", "C-378/07", "2009-04-23", 40),
            porteuse("angelidaki e a", "cjue", "C-378/07", "2008-12-04", 3),
            porteuse("haas", "cjue", "T-3/96", "1998-09-15", 5),
            porteuse("haas", "cjue", "C-338/95", "1997-11-20", 4),
            porteuse(
                "ag2r prevoyance contre beaudout pere et fils sarl",
                "cjue",
                "C-437/09",
                "2011-03-03",
                30,
            ),
            porteuse(
                "union des syndicats de l immobilier et beaudout pere et fils",
                "cjue",
                "C-25/14",
                "2015-12-17",
                10,
            ),
        ]);
        let arret = |docket: &str, date: &str| vec![CaseAddress::new("cjue", docket, date)];
        assert_eq!(
            citee(
                &snap,
                "la Cour de justice, dans l'arrêt Angelidaki e.a., précité"
            ),
            arret("C-378/07", "2009-04-23")
        );
        assert!(citee(&snap, "dans l'affaire Haas, la Cour de justice a jugé").is_empty());
        assert_eq!(
            citee(&snap, "la CJUE, dans l'arrêt Beaudout, précité"),
            arret("C-437/09", "2011-03-03")
        );
        assert_eq!(
            citee(
                &snap,
                "la CJUE, dans l'arrêt BEAUDOUT du 17 décembre 2015, a retenu"
            ),
            arret("C-25/14", "2015-12-17")
        );
    }

    #[test]
    fn la_cour_europeenne_se_cite_d_abord() {
        let snap = CaseNameSnapshot::new([
            porteuse(
                "legros et autres c france",
                "cedh",
                "72173/17",
                "2023-11-09",
                2,
            ),
            porteuse(
                "administration des douanes contre leopold legros et autres",
                "cjue",
                "C-163/90",
                "1992-07-16",
                66,
            ),
        ]);
        let cedh = Citing {
            european: Some("cedh"),
            date: Some("2024-01-01"),
        };
        let corps = format!("{}selon l’arrêt Legros et autres, précité", " ".repeat(400));
        assert_eq!(
            cited_case_name_spans(&corps, &snap, &[], cedh)
                .into_iter()
                .map(|(_, _, r)| rendu(&r))
                .collect::<Vec<_>>(),
            ["cedh|72173/17"]
        );
    }

    #[test]
    fn un_corps_francais_nomme_la_cour_europeenne() {
        let snap = CaseNameSnapshot::new([
            porteuse(
                "procedures penales contre jean louis aubertin isabelle diblanc",
                "cjue",
                "C-29/94",
                "1995-02-16",
                43,
            ),
            porteuse(
                "isabelle gielen contre ministerraad",
                "cjue",
                "C-299/13",
                "2014-02-27",
                4,
            ),
            porteuse("lina rossi c italie", "cedh", "43106/98", "2000-06-22", 0),
        ]);
        assert!(citee(
            &snap,
            "selon la jurisprudence Isabelle, le recours est ouvert"
        )
        .is_empty());
        assert!(citee(&snap, "l'affaire Lina, devant le conseil de prud'hommes").is_empty());
        assert_eq!(
            citee(&snap, "la Cour européenne, dans l'affaire Lina, a jugé").len(),
            1
        );
    }

    #[test]
    fn le_nom_se_lit_dans_l_ordre_ou_le_corps_l_ecrit() {
        let snap = CaseNameSnapshot::new([porteuse(
            "dimos ano liosion grece et autres contre commission des communautes europeennes",
            "cjue",
            "T-85/05",
            "2005-05-23",
            0,
        )]);
        assert!(citee(
            &snap,
            "la CJUE, dans l'affaire Commission/Grèce, l'avocat général"
        )
        .is_empty());
        assert_eq!(
            citee(
                &snap,
                "le Tribunal de l'Union, dans l'affaire Dimos Ano Liosion/Commission"
            )
            .len(),
            1
        );
    }

    #[test]
    fn un_nom_commun_n_ouvre_aucun_nom() {
        let snap = CaseNameSnapshot::new([
            porteuse(
                "mme b contre prefet du rhone",
                "ce",
                "401801",
                "2017-05-10",
                0,
            ),
            porteuse(
                "accord entre la france et monaco",
                "ce",
                "401802",
                "2017-05-10",
                0,
            ),
            porteuse(
                "ministre des affaires etrangeres c x",
                "ce",
                "401803",
                "2017-05-10",
                0,
            ),
            porteuse("chambre arbitrale cas", "ce", "401804", "2017-05-10", 0),
            porteuse("canal robin et godot", "ce", "58502", "1962-10-19", 0),
            porteuse(
                "groupe canal contre office de l union europeenne pour la propriete intellectuelle",
                "cjue",
                "C-500/13",
                "2014-01-16",
                0,
            ),
        ]);
        for texte in [
            "Pour contester cette décision Mme B soutient qu'elle vit en France",
            "DANS L'AFFAIRE ENTRE : Madame X",
            "dans l'affaire ENTRE : Madame X",
            "le Ministère des Affaires Etrangères à Paris",
            "dans une affaire CAS n° 24-082023",
            "pour l'arrêt (Accord, 19 juin 1997)",
            "devant la CJUE, le requérant invoque l'arrêt Canal pour soutenir",
        ] {
            assert!(citee(&snap, texte).is_empty(), "texte {texte:?}");
        }
    }

    #[test]
    fn le_nom_le_plus_long_gagne() {
        let spans = cited_case_name_spans(
            "la jurisprudence Société Tropic Travaux Signalisation ouvre le recours",
            &snap(),
            &[],
            citante(),
        );
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].0, spans[0].1), (17, 53));
    }

    #[test]
    fn les_spans_comptent_en_codepoints() {
        let texte = "où l'intérêt s'apprécie, la jurisprudence Danthony";
        let spans = cited_case_name_spans(texte, &snap(), &[], citante());
        let lu: String = texte.chars().skip(spans[0].0).take(8).collect();
        assert_eq!(lu, "Danthony");
    }
}
