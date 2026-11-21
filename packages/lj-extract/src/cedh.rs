//! A CEDH judgment or decision against France tells, in its facts, the French
//! procedure it follows: the last ruling of the Cour de cassation or the Conseil
//! d'État, by court and date, and the lower decision that ruling reviewed. The
//! link goes out at the ruling's court and date; [`settle`] picks, among the
//! decisions of that court and day, the one the text designates.

use std::collections::HashSet;
use std::sync::LazyLock;

use lj_core::address::CaseAddress;
use regex::Regex;

use crate::chrono::ChronoSnapshot;
use crate::identity::normalize_component;

/// A French court as a CEDH text names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Court {
    Cc,
    Ce,
    Cons,
    Caa,
    Ca,
    Ta,
    /// A first-instance court: tribunal, conseil de prud'hommes, cour d'assises.
    Lower,
    /// The aid bureau, the registry, a president or the parquet of a supreme
    /// court: a date beside them is no ruling.
    Aux,
}

/// What the text says the ruling did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    NonAdmission,
    Irrecevabilite,
    Decheance,
    Desistement,
    Cassation,
    Rejet,
}

impl Said {
    /// The solutions of a candidate the outcome can be.
    fn admits(self, solution: &str) -> bool {
        let ok: &[&str] = match self {
            Said::Rejet => &[
                "REJET",
                "CASSATION_PARTIELLE",
                "IRRECEVABILITE",
                "NON_ADMISSION",
                "DECHEANCE",
                "NON_LIEU_A_STATUER",
                "AUTRE",
                "ANNULATION_PARTIELLE",
            ],
            Said::Cassation => &[
                "CASSATION",
                "CASSATION_PARTIELLE",
                "ANNULATION",
                "ANNULATION_PARTIELLE",
                "REFORMATION",
                "AUTRE",
                "SATISFACTION_PARTIELLE",
                "SATISFACTION_TOTALE",
            ],
            Said::NonAdmission => &["NON_ADMISSION", "REJET", "IRRECEVABILITE", "AUTRE"],
            Said::Irrecevabilite => &["IRRECEVABILITE", "REJET", "NON_ADMISSION", "AUTRE"],
            Said::Decheance => &["DECHEANCE", "REJET", "IRRECEVABILITE", "AUTRE"],
            Said::Desistement => &["DESISTEMENT", "REJET", "AUTRE", "NON_LIEU_A_STATUER"],
        };
        ok.contains(&solution)
    }
}

/// A dated step of the French procedure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub court: Court,
    /// The city the text writes after the court, folded, possibly over-captured.
    pub city: String,
    /// ISO `YYYY-MM-DD`.
    pub date: String,
    pub chamber: Option<&'static str>,
    sentence: String,
    before: String,
    /// A party seizing the court that day: the ruling is still to come.
    filing: bool,
}

/// What a CEDH text says of the French ruling it follows.
#[derive(Debug, Clone)]
pub struct CedhFacts {
    pub ruling: Step,
    pub said: Option<Said>,
    /// Decisions below the ruling: date, court, first word of the city.
    lower: Vec<(String, Court, String)>,
    /// Digits of every number the text writes in the form of the ruling's court.
    numbers: HashSet<String>,
    /// Name tokens of an applicant that is a legal person.
    legal: Vec<String>,
    /// A first name of a natural applicant beside a pseudonym, which is how the
    /// French ruling keeps it.
    first_beside_pseudonym: Option<Regex>,
    /// One run of eight words from each long passage the text quotes.
    quotes: Vec<String>,
    folded: String,
}

/// A decision of the ruling's court and day.
pub struct Candidate {
    pub id: i64,
    /// The solution value, `REJET`, `CASSATION`…
    pub solution: Option<String>,
    pub dockets: Vec<String>,
    /// `(date, court code)` of the decisions its own chronology says it reviews.
    pub attacked: Vec<(String, String)>,
    /// Its text through [`fold`].
    pub text: String,
}

/// The fold both sides are compared in: [`lj_fold`], whitespace runs as one
/// space.
pub fn fold(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        for f in lj_fold::fold_char(c) {
            if f == ' ' && (out.is_empty() || out.ends_with(' ')) {
                continue;
            }
            out.push(f);
        }
    }
    out
}

const MONTHS: [&str; 12] = [
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{1,2})(?:er)?\s+(janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre)\s+((?:19|20)\d{2})\b").unwrap()
});

static COURTS: LazyLock<Vec<(Court, Regex)>> = LazyLock::new(|| {
    const CITY: &str = r"(?: de (?:la |l')?([a-z][a-z' -]{2,30}))?";
    [
        (Court::Cc, r"cour de cassation|chambre criminelle|assemblee pleniere|(?:premiere|deuxieme|troisieme|1re|2e|3e) chambre civile|chambre (?:sociale|commerciale|mixte)".to_string()),
        (Court::Ce, r"conseil d'etat|section du contentieux".to_string()),
        (Court::Cons, r"conseil constitutionnel".to_string()),
        (Court::Caa, format!("cour administrative d'appel{CITY}")),
        (Court::Ca, r"(?:chambre de l'instruction|chambre d'accusation|cour d'appel)(?: de (?:la cour d'appel de )?(?:la |l')?([a-z][a-z' -]{2,30}))?".to_string()),
        (Court::Ta, format!("tribunal administratif{CITY}")),
        (Court::Lower, format!("tribunal (?:de grande instance|correctionnel|judiciaire|d'instance|de commerce|de police|des affaires de securite sociale|pour enfants){CITY}|conseil de prud'hommes|juge d'instruction|cour d'assises")),
    ]
    .into_iter()
    .map(|(c, rx)| (c, Regex::new(&rx).unwrap()))
    .collect()
});

const CHAMBERS: [(&str, &str); 8] = [
    ("crim", "chambre criminelle"),
    ("soc", "chambre sociale"),
    ("com", "chambre commerciale"),
    ("civ1", "(?:premiere|1re) chambre civile"),
    ("civ2", "(?:deuxieme|2e) chambre civile"),
    ("civ3", "(?:troisieme|3e) chambre civile"),
    ("plen", "assemblee pleniere"),
    ("mixte", "chambre mixte"),
];

static CHAMBER_RX: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    CHAMBERS
        .iter()
        .map(|(c, rx)| (*c, Regex::new(rx).unwrap()))
        .collect()
});

fn chamber_of(s: &str) -> Option<&'static str> {
    CHAMBER_RX
        .iter()
        .find(|(_, rx)| rx.is_match(s))
        .map(|(c, _)| *c)
}

static END_FACTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"le droit et la pratique (?:internes|internationa|pertinents)|le droit (?:interne|international|communautaire) pertinent|le droit interne et|le droit international et la pratique|textes internationaux pertinents|le cadre juridique|droit et pratique internes pertinents|\bgriefs?\b|en droit\b|[.»)] questions aux parties").unwrap()
});

static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.;]\s+").unwrap());

static ABBREVIATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\bn|\bm|\bmm|\bme|\bart|\bcass|\bcrim|\bciv|\bbull|\bp|\bch)$").unwrap()
});

static NOT_RULING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:datee?|notifie|signifie|lettre|courrier|enregistre|depose|recu|parvenu|introduit|forme|saisit|saisi)e?s?(?: au requerant| a l'avocat| a la requerante| aux requerants)?(?: le| du| en date du)?\s*$").unwrap()
});

/// A party seizing a court: the date is the filing's.
static FILING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:saisit|saisirent|forma|formerent|se pourvut|se pourvurent|introduisit|introduisirent|deposa|deposerent|interjeta|interjeterent|(?:a|ont) (?:saisi|forme|introduit|depose|interjete))\b").unwrap()
});

static AUX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"aide juridictionnelle|greffe|greffier|procureur general|premier president|avocat general|president d[eu]|parquet general|conseiller rapporteur").unwrap()
});

static CASS_NO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{2})[-.](\d{2})\.?(\d{3})\b").unwrap());
static CE_NO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bn(?:°|o|os|°s)\s*(\d{5,6})\b").unwrap());

static SAID: LazyLock<Vec<(Said, Regex)>> = LazyLock::new(|| {
    [
        (Said::NonAdmission, r"non admis|admission"),
        (Said::Irrecevabilite, r"irrecevable"),
        (Said::Decheance, r"dechu|decheance"),
        (Said::Desistement, r"desistement|donn[ae] acte"),
        (
            Said::Cassation,
            r"\bcass(?:a|e|erent)\b|\bannul(?:a|e|erent)\b",
        ),
        (Said::Rejet, r"rejet|rejeta|confirm"),
    ]
    .into_iter()
    .map(|(s, rx)| (s, Regex::new(rx).unwrap()))
    .collect()
});

static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""\s*([^"]{120,})""#).unwrap());
static EIGHT_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-z]{2,}(?:\s+[a-z]{2,}){7}").unwrap());

static APPLICANT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"AFFAIRE\s+(.+?)\s+c\.\s+FRANCE|présentée par\s+(.+?)\s+contre (?:la )?France|Requêtes? n[°os]+ [\d/ ;et]+\s+(.+?)\s+contre (?:la )?France").unwrap()
});
static LEGAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:societe|ste|sa|sarl|sas|association|federation|syndicat|union|commune|ville|departement|region|comite|confederation|mutuelle|banque|compagnie|groupement|fondation|ordre|clinique|cabinet|editions|parti|eglise)\b").unwrap()
});
const STOP: [&str; 34] = [
    "societe",
    "ste",
    "sarl",
    "sas",
    "association",
    "federation",
    "syndicat",
    "union",
    "nationale",
    "national",
    "francaise",
    "francais",
    "france",
    "autres",
    "des",
    "du",
    "de",
    "la",
    "le",
    "les",
    "et",
    "pour",
    "commune",
    "ville",
    "comite",
    "frere",
    "freres",
    "fils",
    "cie",
    "compagnie",
    "groupe",
    "groupement",
    "cabinet",
    "editions",
];
static FIRST_AFTER_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:requerante?s?|requerants),? (?:m\.|mme|mlle|m) ([a-z][a-z-]{2,})\s").unwrap()
});
static FIRST_BEFORE_SURNAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:par|et) ([A-ZÉ][a-zéèëïöü-]{2,}) [A-ZÉ][A-ZÉÈ' -]{2,}").unwrap()
});

/// The byte at or before `i` that starts a character.
fn floor(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn head(s: &str, n: usize) -> &str {
    &s[..floor(s, n)]
}

/// The sentences of the facts: one ends at a full stop or a semicolon before
/// a lowercase word, a digit, a quote or a parenthesis, unless an abbreviation
/// closes it.
fn sentences(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut last = 0;
    for m in SENTENCE_END.find_iter(s) {
        let next = s[m.end()..].chars().next();
        if !next
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '"' || c == '(')
        {
            continue;
        }
        if m.start() < last || ABBREVIATION.is_match(&s[last..m.start()]) {
            continue;
        }
        out.push(&s[last..m.start()]);
        last = m.end();
    }
    out.push(&s[last..]);
    out
}

/// The day an application was lodged, from the head of the text the CEDH
/// communicates (« introduite le 19 juin 2025 »).
pub fn introduced_on(raw: &str) -> Option<String> {
    INTRODUCED.captures(&fold(head(raw, 3000))).map(|c| iso(&c))
}

static INTRODUCED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"introduite le (\d{1,2})(?:er)?\s+(janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre)\s+((?:19|20)\d{2})\b").unwrap()
});

fn iso(c: &regex::Captures) -> String {
    let month = MONTHS.iter().position(|m| *m == &c[2]).unwrap() + 1;
    let day: u32 = c[1].parse().unwrap();
    format!("{}-{month:02}-{day:02}", &c[3])
}

/// Every dated court of the facts, up to the CEDH's own date: each date goes
/// to the nearest court of its sentence, the one before it on a tie.
fn events(ft: &str, cedh_date: &str) -> Vec<Step> {
    let start = ft.find("en fait").unwrap_or(0);
    let end = END_FACTS
        .find_at(ft, floor(ft, start + 50))
        .map_or(ft.len(), |m| m.start());
    let facts = &ft[start..end.max(start)];
    let mut out = Vec::new();
    for sent in sentences(facts) {
        let dates: Vec<(usize, String)> = DATE
            .captures_iter(sent)
            .map(|c| (c.get(0).unwrap().start(), iso(&c)))
            .collect();
        if dates.is_empty() {
            continue;
        }
        let courts: Vec<(usize, Court, String)> = COURTS
            .iter()
            .flat_map(|(court, rx)| {
                rx.captures_iter(sent).map(move |c| {
                    let city = c.get(1).map_or("", |m| m.as_str().trim());
                    (c.get(0).unwrap().start(), *court, city.to_string())
                })
            })
            .collect();
        if courts.is_empty() {
            continue;
        }
        let chamber = chamber_of(sent);
        for (dpos, date) in dates {
            if date.as_str() > cedh_date {
                continue;
            }
            let (pos, mut court, city) = courts
                .iter()
                .min_by_key(|(p, _, _)| (p.abs_diff(dpos), *p > dpos))
                .cloned()
                .unwrap();
            if matches!(court, Court::Cc | Court::Ce)
                && (AUX.is_match(&sent[floor(sent, pos.saturating_sub(70))..pos])
                    || sent.contains("certificat de non-pourvoi"))
            {
                court = Court::Aux;
            }
            out.push(Step {
                court,
                city,
                chamber: if court == Court::Cc { chamber } else { None },
                before: sent[floor(sent, dpos.saturating_sub(45))..dpos].to_string(),
                sentence: head(sent, 400).to_string(),
                filing: FILING.is_match(&sent[pos.min(dpos)..pos.max(dpos)]),
                date,
            });
        }
    }
    out
}

/// The latest of the steps, the first one written on a tie.
fn latest<'a>(steps: impl Iterator<Item = &'a Step>) -> Option<&'a Step> {
    steps.fold(None, |best: Option<&Step>, s| match best {
        Some(b) if b.date >= s.date => Some(b),
        _ => Some(s),
    })
}

fn city_word(city: &str) -> String {
    let words: Vec<&str> = city.split([' ', '-']).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [first, ..] if !matches!(*first, "la" | "le" | "l") => first.to_string(),
        [_, second, ..] => second.to_string(),
        _ => String::new(),
    }
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

impl CedhFacts {
    /// The facts of a French-language CEDH text against France dated
    /// `cedh_date`, `None` when it names no ruling the link can aim at.
    pub fn read(raw: &str, cedh_date: &str) -> Option<CedhFacts> {
        let ft = fold(raw);
        let top = head(&ft, 3000);
        if !["c. france", "contre la france", "contre france"]
            .iter()
            .any(|state| top.contains(state))
        {
            return None;
        }
        let evs = events(&ft, cedh_date);
        let supreme = latest(evs.iter().filter(|e| {
            matches!(e.court, Court::Cc | Court::Ce) && !NOT_RULING.is_match(&e.before)
        }));
        let ruling = match supreme {
            Some(r) if r.filing => return None,
            Some(r) => r.clone(),
            None => {
                let r = latest(
                    evs.iter()
                        .filter(|e| matches!(e.court, Court::Ca | Court::Caa | Court::Ta)),
                )?;
                if r.court != Court::Caa && r.date.as_str() < "2013" {
                    return None;
                }
                Step {
                    chamber: None,
                    ..r.clone()
                }
            }
        };
        let lower = evs
            .iter()
            .filter(|e| {
                matches!(e.court, Court::Ca | Court::Caa | Court::Ta | Court::Lower)
                    && e.date < ruling.date
            })
            .map(|e| (e.date.clone(), e.court, city_word(&e.city)))
            .collect();
        let numbers = match ruling.court {
            Court::Cc => CASS_NO
                .captures_iter(&ft)
                .map(|c| format!("{}{}{}", &c[1], &c[2], &c[3]))
                .collect(),
            Court::Ce if ft.contains("conseil d'etat") => {
                CE_NO.captures_iter(&ft).map(|c| c[1].to_string()).collect()
            }
            _ => HashSet::new(),
        };
        let (legal, firsts) = applicant(raw);
        let first_beside_pseudonym = (!firsts.is_empty()).then(|| {
            let f = firsts.iter().map(|f| regex::escape(f)).collect::<Vec<_>>().join("|");
            Regex::new(&format!(
                r"\b(?:{f})\b[ ,]{{0,3}}(?:[a-z]\.\.\.|[a-z]\b|\[)|(?:[a-z]\.\.\.|\])[ ,-]{{0,3}}(?:{f})\b"
            ))
            .unwrap()
        });
        let quotes = QUOTE
            .captures_iter(&ft)
            .filter_map(|c| {
                let runs: Vec<&str> = EIGHT_WORDS.find_iter(&c[1]).map(|m| m.as_str()).collect();
                runs.get(runs.len() / 2).map(|s| s.to_string())
            })
            .collect();
        let said = SAID
            .iter()
            .filter_map(|(s, rx)| rx.find(&ruling.sentence).map(|m| (m.start(), *s)))
            .min_by_key(|(pos, _)| *pos)
            .map(|(_, s)| s);
        Some(CedhFacts {
            ruling,
            said,
            lower,
            numbers,
            legal,
            first_beside_pseudonym,
            quotes,
            folded: ft,
        })
    }

    /// The ruling's court and date, its court a referential code.
    pub fn address(&self, snap: &ChronoSnapshot) -> Option<CaseAddress> {
        let r = &self.ruling;
        let code = match r.court {
            Court::Cc => "cc",
            Court::Ce => "ce",
            court => {
                let jt = match court {
                    Court::Caa => "CAA",
                    Court::Ca => "CA",
                    _ => "TA",
                };
                snap.city_prefix_code(jt, &normalize_component(&r.city))?
            }
        };
        Some(CaseAddress::court_and_date(code, &r.date))
    }
}

/// (name tokens of a legal person, first names) of the applicants.
fn applicant(raw: &str) -> (Vec<String>, Vec<String>) {
    let top = head(raw, 3000);
    let name = APPLICANT
        .captures(top)
        .and_then(|c| c.iter().skip(1).flatten().next().map(|m| fold(m.as_str())))
        .unwrap_or_default();
    let legal = if LEGAL.is_match(&name) {
        name.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| t.len() >= 4 && !STOP.contains(t))
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    let mut firsts: Vec<String> = Vec::new();
    let folded = fold(head(raw, 20000));
    let found = FIRST_AFTER_TITLE
        .captures_iter(&folded)
        .map(|c| c[1].to_string())
        .chain(FIRST_BEFORE_SURNAME.captures_iter(top).map(|c| fold(&c[1])));
    for f in found {
        if !matches!(f.as_str(), "france" | "monsieur" | "madame") && !firsts.contains(&f) {
            firsts.push(f);
        }
    }
    (legal, firsts)
}

/// `(kind, first word of the city)` of a referential code.
fn code_parts(code: &str) -> (Court, &str) {
    let (prefix, city) = code.split_once('_').unwrap_or((code, ""));
    let court = match prefix {
        "ca" | "tsa" => Court::Ca,
        "caa" => Court::Caa,
        "ta" => Court::Ta,
        "cc" => Court::Cc,
        "ce" => Court::Ce,
        _ => Court::Lower,
    };
    (court, city.split('_').next().unwrap_or(""))
}

impl CedhFacts {
    /// Does the candidate's head name the applicant?
    fn names_applicant(&self, text: &str) -> bool {
        let top = head(text, 4000);
        (!self.legal.is_empty() && self.legal.iter().all(|t| top.contains(t.as_str())))
            || self
                .first_beside_pseudonym
                .as_ref()
                .is_some_and(|rx| rx.is_match(top))
    }

    /// Does the candidate's chronology name a decision below the ruling the
    /// text names: same day, same kind of court, same city or one the text
    /// names?
    fn chains(&self, c: &Candidate) -> bool {
        c.attacked.iter().any(|(date, code)| {
            let (kind, city) = code_parts(code);
            self.lower.iter().any(|(ld, lk, lc)| {
                ld == date
                    && *lk == kind
                    && if city.is_empty() || lc.is_empty() {
                        city.is_empty() || self.folded.contains(city)
                    } else {
                        lc.starts_with(city) || city.starts_with(lc.as_str())
                    }
            })
        })
    }

    /// The candidate the text designates, among the decisions of the ruling's
    /// court and day: the number; else the decision attacked, narrowed by the
    /// quoted passage then the applicant; else the applicant together with the
    /// quoted passage. The outcome and the chamber the text names exclude a
    /// candidate first.
    pub fn settle(&self, cands: &[Candidate]) -> Option<i64> {
        let kept: Vec<&Candidate> = cands
            .iter()
            .filter(|c| match (self.said, c.solution.as_deref()) {
                (Some(said), Some(sol)) => said.admits(sol),
                _ => true,
            })
            .filter(|c| {
                self.ruling.court != Court::Cc
                    || self.ruling.chamber.is_none()
                    || chamber_of(head(&c.text, 2500))
                        .is_none_or(|ch| Some(ch) == self.ruling.chamber)
            })
            .collect();
        let ids = |f: &dyn Fn(&Candidate) -> bool| -> HashSet<i64> {
            kept.iter().filter(|c| f(c)).map(|c| c.id).collect()
        };
        let single = |s: &HashSet<i64>| (s.len() == 1).then(|| *s.iter().next().unwrap());

        let numbered = ids(&|c| c.dockets.iter().any(|d| self.numbers.contains(&digits(d))));
        if let Some(id) = single(&numbered) {
            return Some(id);
        }
        let quoted = if self.quotes.is_empty() {
            HashSet::new()
        } else {
            ids(&|c| self.quotes.iter().any(|q| c.text.contains(q.as_str())))
        };
        let party = ids(&|c| self.names_applicant(&c.text));
        let chained = if self.lower.is_empty() {
            HashSet::new()
        } else {
            ids(&|c| self.chains(c))
        };
        if !chained.is_empty() {
            let mut s = chained;
            for signal in [&quoted, &party] {
                let narrowed: HashSet<i64> = s.intersection(signal).copied().collect();
                if !narrowed.is_empty() {
                    s = narrowed;
                }
            }
            return single(&s);
        }
        single(&quoted.intersection(&party).copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "CINQUIÈME SECTION AFFAIRE DURAND c. FRANCE (Requête no 1234/05) ARRÊT STRASBOURG 10 juillet 2008. En l'affaire Durand c. France, la Cour... EN FAIT I. LES CIRCONSTANCES DE L'ESPÈCE 1. Le requérant, M. Paul Durand, est né en 1950. ";

    fn cand(id: i64, solution: &str, attacked: &[(&str, &str)], text: &str) -> Candidate {
        Candidate {
            id,
            solution: Some(solution.to_string()),
            dockets: Vec::new(),
            attacked: attacked
                .iter()
                .map(|(d, c)| (d.to_string(), c.to_string()))
                .collect(),
            text: fold(text),
        }
    }

    #[test]
    fn the_ruling_is_the_last_supreme_one_and_the_lower_decision_the_one_it_reviewed() {
        let text = format!(
            "{HEAD}2. Par un arrêt du 4 juillet 1997, la cour d'appel de Douai condamna le requérant. 3. Par un arrêt du 22 octobre 1997, la Cour de cassation (chambre criminelle) rejeta le pourvoi. 4. L'arrêt fut notifié au requérant le 3 novembre 1997. EN DROIT 5. Le grief."
        );
        let f = CedhFacts::read(&text, "2008-07-10").unwrap();
        assert_eq!(f.ruling.court, Court::Cc);
        assert_eq!(f.ruling.date, "1997-10-22");
        assert_eq!(f.ruling.chamber, Some("crim"));
        assert_eq!(f.said, Some(Said::Rejet));
        assert_eq!(
            f.lower,
            [("1997-07-04".to_string(), Court::Ca, "douai".to_string())]
        );
        assert_eq!(
            f.address(&ChronoSnapshot::empty()),
            Some(CaseAddress::court_and_date("cc", "1997-10-22"))
        );
    }

    #[test]
    fn a_case_against_another_state_or_a_date_beside_the_registry_names_no_ruling() {
        let other = "AFFAIRE X c. BELGIQUE EN FAIT 1. Par un arrêt du 22 octobre 1997, la Cour de cassation rejeta le pourvoi.";
        assert!(CedhFacts::read(other, "2008-07-10").is_none());
        let aux = format!(
            "{HEAD}2. Le 22 octobre 1997, le bureau d'aide juridictionnelle près la Cour de cassation rejeta la demande."
        );
        assert!(CedhFacts::read(&aux, "2008-07-10").is_none());
        let law = format!(
            "{HEAD}2. Le 20 mars 2007, la cour d'appel de Paris condamna le requérant. II. LE DROIT ET LA PRATIQUE PERTINENTS 3. Par un arrêt du 26 juin 1995, la Cour de cassation jugea que l'amende était due. EN DROIT"
        );
        assert!(CedhFacts::read(&law, "2008-07-10").is_none());
    }

    #[test]
    fn a_pourvoi_lodged_and_never_ruled_on_names_no_ruling() {
        let pending = format!(
            "{HEAD}2. Le 20 mars 2007, la cour d'appel de Paris condamna le requérant. 3. Le 30 mai 2007, le requérant forma un pourvoi, pendant devant la Cour de cassation. EN DROIT"
        );
        assert!(CedhFacts::read(&pending, "2008-07-10").is_none());
        let ruled = format!(
            "{HEAD}2. Le 30 mai 2007, le requérant forma un pourvoi devant la Cour de cassation. 3. Par un arrêt du 4 avril 2008, la Cour de cassation rejeta le pourvoi. EN DROIT"
        );
        let f = CedhFacts::read(&ruled, "2008-07-10").unwrap();
        assert_eq!(f.ruling.date, "2008-04-04");
    }

    #[test]
    fn a_communicated_case_reads_its_lodging_day_and_its_facts_end_at_the_questions() {
        let text = "CINQUIÈME SECTION Requête no 1234/25 présentée par Paul DURAND contre la France introduite le 1er juin 2025 EXPOSÉ DES FAITS EN FAIT 1. Par un arrêt du 4 avril 2025, la Cour de cassation rejeta le pourvoi du requérant. QUESTIONS AUX PARTIES 1. Le requérant a-t-il épuisé les voies de recours internes, alors que le Conseil d'État, par une décision du 9 mai 2025, a rejeté une demande analogue ?";
        assert_eq!(introduced_on(text), Some("2025-06-01".to_string()));
        let f = CedhFacts::read(text, "2025-10-19").unwrap();
        assert_eq!(f.ruling.court, Court::Cc);
        assert_eq!(f.ruling.date, "2025-04-04");
    }

    #[test]
    fn the_decision_attacked_settles_and_the_outcome_excludes() {
        let text = format!(
            "{HEAD}2. Par un arrêt du 4 juillet 1997, la cour d'appel de Douai condamna le requérant. 3. Par un arrêt du 22 octobre 1997, la Cour de cassation rejeta le pourvoi. EN DROIT"
        );
        let f = CedhFacts::read(&text, "2008-07-10").unwrap();
        let attacked = [("1997-07-04", "ca_douai")];
        let cands = [
            cand(
                1,
                "REJET",
                &[("1997-07-04", "ca_paris")],
                "M. Pierre X... pourvoi",
            ),
            cand(2, "REJET", &attacked, "M. Jean Y... pourvoi"),
            cand(3, "CASSATION", &attacked, "M. Luc Z... pourvoi"),
        ];
        assert_eq!(f.settle(&cands), Some(2));
    }

    #[test]
    fn the_applicant_and_a_quoted_passage_settle_together_never_one_alone() {
        let quoted = "les juges du fond ont souverainement apprecie la valeur et la portee des elements de preuve qui leur etaient soumis et en ont deduit sans contradiction que le prevenu avait commis";
        let text = format!(
            "{HEAD}2. Par un arrêt du 22 octobre 1997, la Cour de cassation rejeta le pourvoi en ces termes : « {quoted} les faits ». EN DROIT"
        );
        let f = CedhFacts::read(&text, "2008-07-10").unwrap();
        let named = "Statuant sur le pourvoi formé par M. Paul D..., contre l'arrêt";
        let other = "Statuant sur le pourvoi formé par M. Henri B..., contre l'arrêt";
        let both = format!("{named} {quoted}");
        assert_eq!(
            f.settle(&[cand(1, "REJET", &[], &both), cand(2, "REJET", &[], other)]),
            Some(1)
        );
        assert_eq!(
            f.settle(&[cand(1, "REJET", &[], named), cand(2, "REJET", &[], other)]),
            None
        );
        let quoted_twice = format!("{other} {quoted}");
        assert_eq!(
            f.settle(&[
                cand(1, "REJET", &[], quoted),
                cand(2, "REJET", &[], &quoted_twice)
            ]),
            None
        );
    }
}
