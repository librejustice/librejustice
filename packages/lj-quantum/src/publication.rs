//! What the table takes from automatic readings: a value two independent
//! readings of the decision write identically, and nothing from a decision
//! whose amounts may not stand on the full base.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::ancrage::{jetons, millimes};
use super::leftovers::{word_at, CURRENCIES};
use super::observation::{euro_rate, observations, Observation};
use super::spreadsheet::{compute, Spreadsheet};
use super::{Poste, Quantum, Victime};

pub const CONTESTED: &str = "le tableur conteste ce poste dans l'une des deux lectures";
pub const UNSURE_BASE: &str = "réduction sans montant réduit : base des montants incertaine";
pub const AGGRAVATION: &str = "aggravation : la décision complète une indemnisation antérieure";
pub const ONE_READING: &str = "une seule des deux lectures publie cette valeur";
pub const UNWRITTEN_TARIFF: &str =
    "tarif que le texte n'écrit pas suivi de sa devise et de son unité";
pub const PRORATED_DFP: &str = "DFP proratisé : la valeur du point ne se lit pas";
pub const UNWRITTEN_AMOUNT: &str = "montant que le texte n'écrit pas";
pub const UNWRITTEN_AGE: &str = "âge que le texte n'écrit pas suivi de « ans »";
pub const UNWRITTEN_RATE: &str = "taux de DFP que le texte n'écrit pas suivi de « % »";
pub const ACQUIRED: &str =
    "somme déjà versée : le poste additionne ce que la décision ne liquide pas";
pub const PROVISION: &str =
    "provision : un montant de la décision n'est écrit que comme une avance";
pub const CAPPED: &str = "montant plafonné par la demande : la cour ne l'évalue pas au-delà";
pub const GLOBAL: &str = "évaluation globale : le montant répare plusieurs postes";
pub const JOINT: &str = "somme commune à plusieurs proches";
pub const PERIOD_COTATION: &str =
    "cotation d'une période seulement : le montant couvre aussi les autres";
pub const PRORATED_DFT: &str = "DFT proratisé : le tarif plein ne se lit pas";
pub const SHARE: &str = "montant d'une part du dommage : la cotation ou le taux mesure le tout";
pub const SHARED_TARIFF: &str = "tarif d'une part du dommage : le tarif plein ne se lit pas";
pub const RESIDUAL_TARIFF: &str = "tarif d'un reste à charge : le prix plein ne se lit pas";
pub const SEVERAL_DEATHS: &str = "somme pour le décès de plusieurs proches";
pub const UNCAUSED_DEATH: &str =
    "décès que la décision ne rattache pas aux faits : la catégorie du proche ne se lit pas";

/// A row the table takes: a value proven beyond its own reading, an abscissa,
/// and no reason to stay off.
pub fn on_table(o: &Observation) -> bool {
    matches!(o.preuve.as_deref(), Some("confirmee" | "sommee"))
        && o.axe.is_some()
        && o.hors_table.is_none()
}

/// What the table publishes from a row, to the cent, with every field that
/// files it: two readings agree on a row when they publish the same fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Fact {
    /// The point value, written or implied by the amount over the points: a
    /// reading that writes it and one that does not publish the same cell.
    Grille {
        poste: String,
        point: Option<i64>,
        points: Option<i64>,
        age: Option<i64>,
    },
    Unite {
        poste: String,
        prix: i64,
        unite: Option<String>,
        nature_aide: Option<String>,
        mode_emploi: Option<String>,
        specialisee: Option<bool>,
        nuit: Option<bool>,
    },
    Cotation {
        poste: String,
        cotation: i64,
        montant: Option<i64>,
    },
    Categorie {
        poste: String,
        categorie: String,
        deces: Option<bool>,
        montant: Option<i64>,
    },
}

fn cents(x: Option<f64>) -> Option<i64> {
    x.map(|v| (v * 100.0).round() as i64)
}

/// The fact of a row that has an axis. The axis rule guarantees the field
/// each axis is placed by.
pub fn fact(o: &Observation) -> Option<Fact> {
    let poste = o.poste.clone();
    Some(match o.axe.as_deref()? {
        "grille" => Fact::Grille {
            poste,
            point: cents(o.valeur_unitaire.or_else(|| {
                let points = o.quantite.filter(|q| *q > 0.0 && o.taux_pct.is_none())?;
                o.montant.map(|m| m / points)
            })),
            points: cents(o.quantite),
            age: o.age.map(|a| a.round() as i64),
        },
        "unite" => Fact::Unite {
            poste,
            prix: cents(o.valeur_unitaire).expect("a tariff row carries its price"),
            unite: o.unite.clone(),
            nature_aide: o.nature_aide.clone(),
            mode_emploi: o.mode_emploi.clone(),
            specialisee: o.specialisee,
            nuit: o.nuit,
        },
        "cotation" => Fact::Cotation {
            poste,
            cotation: cents(o.cotation).expect("a cotation row carries its cotation"),
            montant: cents(o.montant),
        },
        "categorie" => Fact::Categorie {
            poste,
            categorie: o
                .categorie
                .clone()
                .expect("a categorical row carries its category"),
            deces: o.deces,
            montant: cents(o.montant),
        },
        other => panic!("axis {other} is not one the table knows"),
    })
}

/// A victim a partage or a lost chance cuts, with no reduced figure written on any
/// poste: a reduced amount could enter the table as a full one.
pub fn unsure_base(q: &Quantum) -> bool {
    q.victimes.iter().any(|v| {
        let cut = |p: Option<f64>| p.is_some_and(|p| p < 100.0);
        (cut(v.partage_responsabilite_pct) || cut(v.perte_de_chance_pct))
            && !v.postes.iter().any(Poste::writes_reduction)
    })
}

/// The decision repairs an aggravation, or the part of a prior state the
/// event aggravated: its amounts complete an earlier award or leave a share
/// out, and neither is a full valuation.
static AGGRAVATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"au titre de (l'|cette |son |leur )?aggravation|(resultant|decoulant|du fait|consecuti\w*|imputables?|lie\w*|suite) (de |a |d')?(cette |l'|son |leur |une |la )?aggravation|post[- ]?aggravation|(indemnis\w*|repar\w*|liquid\w*|evalu\w*) (de |des |du |en )?(l'|son |cette |leur )?(prejudices? (subis |resultant )?(du fait )?(de |d')?(l'|cette )?)?aggravation|en aggravation|\d\s?% d'aggravation|(propre|posterieure?s?|anterieure?s?) a (l'|cette )aggravation|(date|point de depart) de (l'|cette )aggravation|nouvelle aggravation|consolidation de (l'|cette |son )aggravation",
    )
    .expect("aggravation pattern")
});

pub fn repairs_aggravation(folded: &str) -> bool {
    AGGRAVATION_RE.is_match(folded)
}

/// How the text words each unit right after a price, on folded characters.
static UNIT_WORDS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    const ANNUAL: &str = r"par an\b|annuel|/ ?an\b|par annee|l'an\b";
    [
        (
            "jour",
            r"par jour|/ ?jours?\b|journali|quotidien|le jour|par journee|/ ?j\b|chaque jour"
                .to_string(),
        ),
        ("heure", r"par heure|l'heure|horaire|/ ?h\b".to_string()),
        ("mois", r"par mois|mensuel|/ ?mois".to_string()),
        ("an", ANNUAL.to_string()),
        (
            "semaine",
            r"par semaine|hebdomadaire|/ ?semaine".to_string(),
        ),
        ("seance", r"par seance|la seance|/ ?seance".to_string()),
        (
            "euro_de_rente",
            format!(r"de rente|prix de l'euro|coefficient|capitalis|{ANNUAL}"),
        ),
        ("point", r"le point|du point|par point|point d".to_string()),
    ]
    .into_iter()
    .map(|(u, rx)| (u, Regex::new(&rx).expect("unit pattern")))
    .collect()
});

/// Or it names the unit first, as an adjective a few words before the price:
/// « au taux horaire de 20 euros », « une base journalière de 25 euros ». Not
/// « par jour », which in « 3 heures par jour au taux de 19 euros » counts hours.
static UNIT_BEFORE: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let annual = r"annuel|annuelle";
    [
        ("jour", r"journalier|journaliere|quotidien|quotidienne"),
        ("heure", r"horaire"),
        ("mois", r"mensuel|mensuelle"),
        ("an", annual),
        ("semaine", r"hebdomadaire"),
        ("euro_de_rente", annual),
    ]
    .into_iter()
    .map(|(u, rx)| {
        let rx = format!(r"\b({rx})( +[a-z']+){{0,3}} *$");
        (u, Regex::new(&rx).expect("unit pattern"))
    })
    .collect()
});

/// A court writes a tariff as its price, a currency, then its unit: « 19 euros
/// de l'heure », « 873 € par mois ». Beyond this reach the unit belongs to
/// another figure: loosened to 60, a date « 20 avril » passes as 20 a day.
const TARIFF_REACH: usize = 28;

/// No court prices a total deficit under 5 euros a day: the lowest full rate read is 300 a month.
const LOWEST_FULL_DFT: f64 = 5.0;

const SENTENCE_REACH: usize = 500;
/// How far back the passage of an amount runs when no amount comes before it.
const PASSAGE_REACH: usize = 800;

static PROVISIONAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"provision").expect("provision"));
/// A provision the sentence deducts from the award does not make it one.
static DEDUCTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"deduction|dedui|deduit|imput|deja (verse|percu|regle)|reste du|restant du")
        .expect("deduction")
});
/// How a court says the claim caps the amount it awards.
static CAPPED_BY_CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"ultra petita|dans la limite (de (la |sa |leur )?demande|des demandes|de (ses|leurs) pretentions|de la somme (demandee|reclamee|sollicitee)|de ce qui (est|a ete) (demande|sollicite|reclame))|(tenant compte|compte tenu|au regard) du quantum (de la |des )?demande|\bn(e |')(sollicit|demand|reclam)\w* que\b|au-dela de (la |sa |leur )?demande|superieure? a (celle|la somme) (demandee|sollicitee|reclamee)",
    )
    .expect("capped")
});
/// Said in the sentence after the amount, this still caps it.
static HELD_BY_CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"tenue?s? par (les |ses |leurs )?(pretentions|demandes)").expect("held")
});
/// A total deficit or a full base named beside a daily price, a sum « au total » aside.
static FULL_DEGREE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:(au|un|le|du|montant|somme) )?tota(?:l|le|ux|les)\b|100 ?%|taux plein|\bbase\b",
    )
    .expect("full degree")
});
/// A class or a partial rate labelling the price right before it: « classe 2 : 6,25 € ».
static CLASS_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\bclasses? (?:[1-4]|i{1,3}|iv)\b|\bpartiel(?:le)?s?\b|\bdftp\b|\bgtp\b|\b\d{1,2}(?:[.,]\d+)? ?%)\)? ?: ?$")
        .expect("class before")
});
/// Or right after it: « 8,25 euros par jour pour un taux de 25 % ».
static CLASS_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ ?(?:€|euros?|eur\b)? ?(?:(?:par |le |/ ?)(?:jour|mois|semaine)s?|mensuels?|journaliers?)? ?,? ?(?:pour|en) (?:un |une |le |la |l')?(?:taux|classe|deficit|dft|gene)\w* (?:de |a |d'un |fonctionnel temporaire )?(?:\d{1,2}(?:[.,]\d+)? ?%|(?:[1-4]|i{1,3}|iv)\b|partiel)")
        .expect("class after")
});
/// A rate that multiplies the price where it is written makes it the full one.
static RATE_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ ?(?:€|euros?|eur\b)? ?(?:(?:par |le |/ ?)(?:jour|mois|semaine)s?|mensuels?|journaliers?)?\W{0,3}(?:(?:x|\*|×) ?\d+ ?(?:jours?|mois|semaines?)\W{0,3})?(?:x|\*|×|/) ?(?:\d{1,3}(?:[.,]\d+)? ?%|0[.,]\d+)")
        .expect("rate after")
});
static RATE_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\d{1,3}(?:[.,]\d+)? ?%|0[.,]\d+) ?(?:(?:x|\*|×) ?\d+ ?(?:jours?|mois|semaines?) ?)?(?:x|\*|×|de|d'une base de|sur (?:la |une )?base (?:journaliere |mensuelle )?de) ?\(?$")
        .expect("rate before")
});
/// How a court says an amount repairs only the share one fault, infection or co-debtor owes.
static SHARE_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"seules? (les )?consequences|\b(liee?s?|imputables?|dus|dues?) a (la |les )?seules?\b|\ben lien avec (la |les )?seules?\b|\bdu fait (de la |des )?seules?\b|imputabilite d'(un|une) (tiers|quart|moitie|partie)|part (de|imputable a) l'etat anterieur|auraient ete (evaluee?s?|cotee?s?|fixee?s?|estimee?s?|de) (a )?\d|\bdont (seulement )?\d+([.,]\d+)? ?(/ ?7|sur 7|%) (sont|est|seraient|relev|imputab|au titre|result|decoul|lie|dus?)|a parts egales|pour moitie|\b(un|le|du|ce|son|leur) surcroit",
    )
    .expect("share")
});
/// Or that part of the damage predates the facts.
static PRIOR_SHARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"une partie (des|de|du|d')[^.;]{0,120}\bpreexist|\b(identiques?|egaux|egales?|inferieure?s?)( ou (identiques?|egaux|egales?|inferieure?s?))? au taux d'(incapacite|invalidite)",
    )
    .expect("prior share")
});
/// What a payer leaves the victim to pay.
static LEFT_TO_PAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(reste|restent|restant|restee?s?) a (sa |la |leur )?charge")
        .expect("left to pay")
});
const LEFT_TO_PAY_REACH: usize = 60;
/// A rate or a note as the text writes it: « 12,5 % », « 3/7 », « 4 sur une échelle ».
static RATE_OR_NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{1,3}(?:,\d+)?) ?(?:%|/ ?7|sur 7|sur une echelle)").expect("rate or note")
});
/// A note the text splits right after writing it: « 6 sur 7 dont 3 sur 7 relèvent ».
static SPLIT_NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[^.;0-9]{0,40}\bdont (seulement )?\d+([.,]\d+)? ?(/ ?7|sur 7)").expect("split")
});
/// The text says the victim died of another cause than the facts. A match
/// after « avant » speaks of something done before the death.
static UNCAUSED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\bavant )?\b(?:decede\w*\b[^.;]{0,100}|(?:le|son|leur|du|au) deces\b(?: de [^;,]{1,40}?)?,? (?:est |etait |etant )?)(?:sans (?:aucun )?(?:lien|rapport|relation)\b|etrangere?s? (?:a|aux)\b|non imputables?\b|n'est pas imputable|independante?s? (?:de|des|du)\b|d'une autre cause|de cause etrangere)",
    )
    .expect("uncaused death")
});
/// What an award to a relative says of a victim who survives.
static SURVIVAL_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bsurvi(?:e|vante?s?)\b|handicap|a la vue|decheance|etat de sante|souffrances endurees par|degradation")
        .expect("survival")
});
/// What an award to a relative says of a death.
static DEATH_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\bdece(?:s|de|dee|des|dees)\b|\bdisparition\b|\bdisparue?s?\b|\bmorte?s?\b|\bdeuil\b|\bsuicide\b|\btuee?s?\b|\b(?:perte (?:de|d')|perdu) ?(?:son|sa|leur|ses|leurs|un|une|l') ?(?:epoux|epouse|mari|femme|conjoint|compagn\w+|fils|fille|enfant|pere|mere|parent|frere|s(?:oe|o)ur|proche|petit)",
    )
    .expect("death")
});
/// The death of several relatives, repaired by one sum.
static SEVERAL_DEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(deces|disparition|perte|mort) (de ses|de leurs|des) ((deux|trois|quatre|cinq|six|\d) )?(enfants|fils|filles|freres|s(oe|o)urs|parents|petits-enfants|grands-parents|proches)\b",
    )
    .expect("several dead")
});
static GLOBAL_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bglobal(e|es|ement)?\b|confondu").expect("global"));
/// « Global » also says all-inclusive for one poste: it prices several only
/// beside several heads.
static SEVERAL_HEADS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(ces|les|des|deux|leurs|tous) (\w+ )?(postes|prejudices|chefs)\b|temporaire et (permanent|definitif)")
        .expect("heads")
});
/// A sum for costs is often global, and shares its figure with a poste.
static COSTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"700|761-1|irrepetible|depens").expect("costs"));
static SEVERAL_RELATIVES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(les|des|aux|leurs) (parents|enfants|epoux|consorts|freres|s(oe|o)urs|proches|grands-parents|petits-enfants|beaux-parents|ayants[ -]droits?)\b",
    )
    .expect("relatives")
});
static EACH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"chacun|chaque|respectivement|par personne|par tete|individuellement")
        .expect("each")
});
static ON_SEVEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d+(?:[.,]\d+)?) ?(?:/ ?7|sur 7|sur une echelle (?:de (?:0|1|un|zero) a )?7)(?:[^0-9]|$)")
        .expect("cotation")
});
/// What bounds a note in time right after it: « 3/7 jusqu'au 31 mai 2022 ».
static FOR_A_PERIOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\W{0,3}(jusqu|durant|pendant|a compter|a partir|puis|du (\d|1er))")
        .expect("period")
});
/// What opens a later period of the temporary damage before its note.
static LATER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"a compter|a partir|puis|ensuite").expect("later"));
/// How a court names each nature of human aid when it names it.
static NATURE_WORDS: LazyLock<[(&'static str, Regex); 2]> = LazyLock::new(|| {
    [
        ("active", Regex::new(r"\bactives?\b").expect("active")),
        (
            "presence_responsable",
            Regex::new(r"surveillance|presence|passive|veille").expect("presence"),
        ),
    ]
});

/// A decision's text, tokenised once for every age and tariff of its records.
pub struct Written {
    chars: Vec<char>,
    folded: Vec<char>,
    numbers: HashMap<i64, Vec<(usize, usize)>>,
    /// Every number the text follows with a currency, in reading order.
    amounts: Vec<(usize, usize)>,
    aggravation: bool,
    uncaused_death: bool,
}

impl Written {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let folded: Vec<char> = chars.iter().copied().map(lj_fold::fold_one).collect();
        let mut numbers: HashMap<i64, Vec<(usize, usize)>> = HashMap::new();
        for j in jetons(&chars) {
            numbers.entry(j.valeur).or_default().push((j.debut, j.fin));
        }
        let whole = lj_fold::fold(text);
        let uncaused_death = UNCAUSED.captures_iter(&whole).any(|c| c.get(1).is_none());
        let mut written = Self {
            chars,
            folded,
            numbers,
            amounts: Vec::new(),
            aggravation: repairs_aggravation(&whole),
            uncaused_death,
        };
        let mut amounts: Vec<(usize, usize)> = written
            .numbers
            .values()
            .flatten()
            .copied()
            .filter(|&(_, end)| written.currency_end(end).is_some())
            .collect();
        amounts.sort_unstable();
        amounts.dedup();
        written.amounts = amounts;
        written
    }

    /// The text writes this number somewhere, currency or not.
    pub fn writes(&self, amount: f64) -> bool {
        self.numbers.contains_key(&millimes(amount))
    }

    /// Where the text writes this amount followed by a currency.
    fn written(&self, amount: f64) -> Vec<(usize, usize)> {
        self.numbers
            .get(&millimes(amount))
            .into_iter()
            .flatten()
            .copied()
            .filter(|&(_, end)| self.currency_end(end).is_some())
            .collect()
    }

    /// A line break, a semicolon, or a full stop a capital follows.
    fn ends_sentence(&self, i: usize) -> bool {
        match self.chars[i] {
            '\n' | ';' => true,
            '.' => {
                self.chars.get(i + 1).is_some_and(|c| c.is_whitespace())
                    && self
                        .chars
                        .get(i + 2)
                        .is_some_and(|c| c.is_uppercase() || *c == '\n')
            }
            _ => false,
        }
    }

    fn sentence_bounds(&self, start: usize, end: usize) -> (usize, usize) {
        let floor = start.saturating_sub(SENTENCE_REACH);
        let ceil = (end + SENTENCE_REACH).min(self.chars.len());
        let lo = (floor..start)
            .rev()
            .find(|&i| self.ends_sentence(i))
            .map_or(floor, |i| i + 1);
        let hi = (end..ceil).find(|&i| self.ends_sentence(i)).unwrap_or(ceil);
        (lo, hi)
    }

    fn sentences(&self, amount: f64) -> Vec<String> {
        self.written(amount)
            .into_iter()
            .map(|(start, end)| {
                let (lo, hi) = self.sentence_bounds(start, end);
                self.folded[lo..hi].iter().collect()
            })
            .collect()
    }

    /// Every sentence that writes this amount speaks of a provision it grants.
    pub fn provisional(&self, amount: f64) -> bool {
        let s = self.sentences(amount);
        !s.is_empty()
            && s.iter()
                .all(|s| PROVISIONAL.is_match(s) && !DEDUCTED.is_match(s))
    }

    /// Some sentence writing this amount, or the next one if it writes none, says
    /// the court may not award more than asked.
    pub fn capped(&self, amount: f64) -> bool {
        self.written(amount).into_iter().any(|(start, end)| {
            let (lo, hi) = self.sentence_bounds(start, end);
            let own: String = self.folded[lo..hi].iter().collect();
            let next_held = (hi + 1 < self.chars.len())
                .then(|| self.sentence_bounds(hi + 1, hi + 1))
                .is_some_and(|(from, to)| {
                    !self.amounts.iter().any(|&(s, _)| s >= from && s < to)
                        && HELD_BY_CLAIM.is_match(&self.folded[from..to].iter().collect::<String>())
                });
            CAPPED_BY_CLAIM.is_match(&own) || HELD_BY_CLAIM.is_match(&own) || next_held
        })
    }

    /// Some place labels this daily price a class's or a partial deficit's, and no
    /// place names it total or a base; a place a rate multiplies says nothing.
    pub fn partial_rate(&self, price: f64) -> bool {
        let degrees: Vec<bool> = self
            .spans(price)
            .into_iter()
            .filter_map(|[(from, start), (end, to), _]| {
                let before: String = self.folded[from..start].iter().collect();
                let after: String = self.folded[end..to].iter().collect();
                let tail: String = self.folded[start.saturating_sub(30).max(from)..start]
                    .iter()
                    .collect();
                if RATE_BEFORE.is_match(&before)
                    || RATE_AFTER.is_match(&after)
                    || before.trim_end().ends_with('=')
                {
                    None
                } else if (CLASS_BEFORE.is_match(&tail) && !tail.contains("total"))
                    || CLASS_AFTER.is_match(&after)
                {
                    Some(true)
                } else {
                    FULL_DEGREE
                        .captures_iter(&format!("{before} {after}"))
                        .any(|c| c.get(1).is_none())
                        .then_some(false)
                }
            })
            .collect();
        degrees.contains(&true) && !degrees.contains(&false)
    }

    /// Some passage leading to this amount says it repairs a share of the damage,
    /// and no sentence saying so writes the row's own rate or note after it.
    pub fn share_only(&self, amount: f64, abscissa: Option<f64>) -> bool {
        let own = abscissa.map(|a| (a * 100.0).round() as i64);
        let (mut shared, mut held) = (false, false);
        for [_, after, reach] in self.spans(amount) {
            for passage in [self.lines(reach), self.lines(after)] {
                let professional = |m: &regex::Match| {
                    passage[m.end()..]
                        .chars()
                        .take(40)
                        .collect::<String>()
                        .contains("professionnel")
                };
                for m in SHARE_ONLY
                    .find_iter(&passage)
                    .filter(|m| !professional(m))
                    .chain(PRIOR_SHARE.find_iter(&passage))
                {
                    let rest = &passage[m.start()..];
                    shared = true;
                    let stop = [rest.find(['\n', ';']), rest.find(". ")]
                        .into_iter()
                        .flatten()
                        .min()
                        .unwrap_or(rest.len());
                    held |= own.is_some_and(|own| {
                        RATE_OR_NOTE.captures_iter(&rest[..stop]).any(|c| {
                            c[1].replace(',', ".")
                                .parse::<f64>()
                                .is_ok_and(|n| (n * 100.0).round() as i64 == own)
                        })
                    });
                }
            }
        }
        shared && !held
    }

    /// Some sentence writes this price right after saying it is a share of the damage.
    pub fn shared_price(&self, price: f64) -> bool {
        self.spans(price).into_iter().any(|[before, _, _]| {
            let before = self.lines(before);
            SHARE_ONLY.is_match(&before) || PRIOR_SHARE.is_match(&before)
        })
    }

    /// Some place writing this tariff names it what a payer leaves the victim to pay.
    pub fn residual_price(&self, price: f64, unite: &str) -> bool {
        let places = self.tariff_places(price, unite);
        self.spans(price)
            .into_iter()
            .filter(|[(_, start), (end, _), _]| places.contains(&(*start, *end)))
            .any(|[(from, start), (end, to), _]| {
                let before: String = self.folded
                    [start.saturating_sub(LEFT_TO_PAY_REACH).max(from)..start]
                    .iter()
                    .collect();
                let after: String = self.folded[end..to.min(end + LEFT_TO_PAY_REACH)]
                    .iter()
                    .collect();
                LEFT_TO_PAY.is_match(&before) || LEFT_TO_PAY.is_match(&after)
            })
    }

    /// The text writes this note and splits it at once into a share.
    pub fn split_note(&self, cotation: f64) -> bool {
        let own = (cotation * 100.0).round() as i64;
        let text: String = self.folded.iter().collect();
        ON_SEVEN.captures_iter(&text).any(|c| {
            let note = c[1].replace(',', ".").parse::<f64>().ok();
            note.is_some_and(|n| (n * 100.0).round() as i64 == own)
                && SPLIT_NOTE.is_match(&text[c.get(0).expect("note").end()..])
        })
    }

    /// Some passage leading to this amount names the death of several relatives.
    pub fn several_dead(&self, amount: f64) -> bool {
        self.spans(amount).into_iter().any(|[before, _, reach]| {
            SEVERAL_DEAD.is_match(&self.lines(before)) || SEVERAL_DEAD.is_match(&self.lines(reach))
        })
    }

    /// Some passage leading to this amount speaks of the victim's survival, and
    /// none of a death.
    pub fn survival(&self, amount: f64) -> bool {
        let passages: Vec<String> = self
            .spans(amount)
            .into_iter()
            .map(|[_, after, reach]| format!("{} {}", self.lines(reach), self.lines(after)))
            .collect();
        passages.iter().any(|p| SURVIVAL_WORDS.is_match(p))
            && !passages.iter().any(|p| DEATH_WORDS.is_match(p))
    }

    /// Around each place the text writes this amount: its sentence since the amount
    /// before it, its sentence until the next one, and a longer lead.
    fn spans(&self, amount: f64) -> Vec<[(usize, usize); 3]> {
        self.written(amount)
            .into_iter()
            .map(|(start, end)| {
                let previous = self
                    .amounts
                    .iter()
                    .filter(|&&(s, _)| s < start)
                    .map(|&(_, e)| e)
                    .max()
                    .unwrap_or(0);
                let (sentence_start, sentence_end) = self.sentence_bounds(start, end);
                let to = self
                    .amounts
                    .iter()
                    .filter(|&&(s, _)| s >= end)
                    .map(|&(s, _)| s)
                    .min()
                    .unwrap_or(self.chars.len())
                    .min(sentence_end)
                    .max(end);
                [
                    (previous.max(sentence_start).min(start), start),
                    (end, to),
                    (
                        previous.max(start.saturating_sub(PASSAGE_REACH)).min(start),
                        start,
                    ),
                ]
            })
            .collect()
    }

    /// The folded span, its line breaks kept: a line of a list ends there.
    fn lines(&self, (from, to): (usize, usize)) -> String {
        (from..to)
            .map(|i| match self.chars[i] {
                '\n' => '\n',
                _ => self.folded[i],
            })
            .collect()
    }

    /// Every sentence leading to this amount prices several postes at once.
    pub fn global(&self, amount: f64) -> bool {
        let passages: Vec<[String; 3]> = self
            .spans(amount)
            .into_iter()
            .map(|s| s.map(|(from, to)| self.folded[from..to].iter().collect()))
            .collect();
        !passages.is_empty()
            && passages.iter().all(|[before, after, _]| {
                !PROVISIONAL.is_match(before)
                    && !COSTS.is_match(&format!("{before} {after}"))
                    && SEVERAL_HEADS.is_match(before)
                    && (GLOBAL_WORD.is_match(before) || after.contains("confondu"))
            })
    }

    /// Every passage writing this amount beside its cotation bounds that note
    /// to one period, and gives another period another note.
    pub fn cotation_changes(&self, amount: f64, cotation: f64) -> bool {
        let own = (cotation * 100.0).round() as i64;
        let near: Vec<bool> = self
            .spans(amount)
            .into_iter()
            .filter_map(|[_, after, reach]| {
                let passage = format!("{} {}", self.lines(reach), self.lines(after));
                let notes: Vec<(i64, bool)> = ON_SEVEN
                    .captures_iter(&passage)
                    .filter_map(|c| {
                        let whole = c.get(0)?;
                        let lead = &passage[..whole.start()];
                        let sentence = &lead[lead.rfind(['\n', ';', '.']).map_or(0, |i| i + 1)..];
                        let bounded = FOR_A_PERIOD.is_match(&passage[whole.end()..])
                            || (LATER.is_match(sentence) && sentence.contains("temporaire"));
                        let note = c[1].replace(',', ".").parse::<f64>().ok()?;
                        Some(((note * 100.0).round() as i64, bounded))
                    })
                    .collect();
                notes.iter().any(|&(n, _)| n == own).then(|| {
                    notes.iter().any(|&(n, b)| n == own && b)
                        && notes.iter().any(|&(n, b)| n != own && b)
                })
            })
            .collect();
        !near.is_empty() && near.iter().all(|b| *b)
    }

    /// Whether the sentence of this price, or the one before it, names this nature
    /// of aid; nothing for a nature the courts have no word for.
    pub fn names_nature(&self, price: f64, nature: &str) -> Option<bool> {
        let (_, words) = NATURE_WORDS.iter().find(|(n, _)| *n == nature)?;
        Some(self.written(price).into_iter().any(|(start, end)| {
            let (lo, hi) = self.sentence_bounds(start, end);
            let (before, _) = self.sentence_bounds(lo.saturating_sub(1), lo.saturating_sub(1));
            words.is_match(&self.folded[before.min(lo)..hi].iter().collect::<String>())
        }))
    }

    /// The text writes this age as a number followed by « ans ».
    pub fn age(&self, age: f64) -> bool {
        self.numbers
            .get(&millimes(age))
            .into_iter()
            .flatten()
            .any(|&(_, end)| {
                let mut i = end;
                while self.folded.get(i) == Some(&' ') {
                    i += 1;
                }
                word_at(&self.folded, i, "ans")
            })
    }

    /// The text writes this rate as a number followed by « % », « pour cent », « points »,
    /// or by the product sign of « 13 x 2 300 € ». Some sources write « 12'% » for the
    /// thin space they lost.
    pub fn percent(&self, rate: f64) -> bool {
        self.numbers
            .get(&millimes(rate))
            .into_iter()
            .flatten()
            .any(|&(_, end)| {
                let mut i = end;
                while matches!(self.folded.get(i), Some(' ' | '\'')) {
                    i += 1;
                }
                matches!(self.chars.get(i), Some('%' | '×' | '*'))
                    || ["pour cent", "points", "point", "x"]
                        .iter()
                        .any(|w| word_at(&self.folded, i, w))
            })
    }

    /// Where the currency after a number ends, if one follows it.
    fn currency_end(&self, end: usize) -> Option<usize> {
        let mut i = end;
        while self.folded.get(i) == Some(&' ') {
            i += 1;
        }
        if self.chars.get(i) == Some(&'€') {
            return Some(i + 1);
        }
        CURRENCIES
            .iter()
            .filter(|w| word_at(&self.folded, i, w))
            .map(|w| i + w.chars().count())
            .max()
    }

    /// The text writes this price, in the record's currency, followed by a
    /// currency, the unit's words within reach after it or just before it.
    pub fn tariff(&self, price: f64, unite: &str) -> bool {
        !self.tariff_places(price, unite).is_empty()
    }

    fn tariff_places(&self, price: f64, unite: &str) -> Vec<(usize, usize)> {
        let Some((_, words)) = UNIT_WORDS.iter().find(|(u, _)| *u == unite) else {
            return Vec::new();
        };
        let before = UNIT_BEFORE
            .iter()
            .find(|(u, _)| *u == unite)
            .map(|(_, r)| r);
        self.numbers
            .get(&millimes(price))
            .into_iter()
            .flatten()
            .copied()
            .filter(|&(start, end)| {
                let Some(c) = self.currency_end(end) else {
                    return false;
                };
                let reach = (end + TARIFF_REACH).min(self.folded.len());
                let lead = start.saturating_sub(TARIFF_REACH + 12);
                (c < reach && words.is_match(&self.folded[c..reach].iter().collect::<String>()))
                    || before.is_some_and(|r| {
                        r.is_match(&self.folded[lead..start].iter().collect::<String>())
                    })
            })
            .collect()
    }
}

/// A poste the reading labels as given to several relatives, on the only
/// victim of that link: one sum for all of them, and no one's own.
fn joint(q: &Quantum, v: &Victime, p: &Poste) -> bool {
    let label = p
        .libelle_decision
        .as_deref()
        .map(lj_fold::fold)
        .unwrap_or_default();
    SEVERAL_RELATIVES.is_match(&label)
        && !EACH.is_match(&label)
        && q.victimes.iter().filter(|w| w.lien == v.lien).count() == 1
}

/// The postes whose cells hold two values in this reading, by victim and poste
/// ordinal: a poste whose own arithmetic disputes it publishes nothing.
fn contested(t: &Spreadsheet) -> HashSet<(i16, i16)> {
    t.verdict()
        .conflicts
        .iter()
        .filter_map(|c| {
            let (v, rest) = c.cell.strip_prefix('v')?.split_once(".p")?;
            let p = rest.split('.').next()?;
            Some((v.parse().ok()?, p.parse().ok()?))
        })
        .collect()
}

/// The first reading's rows, those the table would take given a reason to
/// stay off when a guard refuses them. The reasons, in the order they apply:
/// a base either reading leaves unsure, an aggravation, an amount the text
/// writes only as a provision, a poste the spreadsheet disputes, a fact the
/// second reading does not publish identically, a sum already paid, an amount
/// the claim caps, one for a share of the damage, a tariff for a share or for
/// what a payer leaves the victim to pay, an amount for several postes, several
/// relatives or several deaths at once, a death the text does not lay on the
/// facts, a cotation for one period, a prorated DFP or DFT, an amount, a DFP
/// rate, an age or a tariff the text does not write. A kept tariff says whether
/// the text names the nature of its aid; a kept point value the text does not
/// write is dropped, the amount over the points giving the same cell.
pub fn publish(first: &Quantum, second: &Quantum, text: &Written) -> Vec<Observation> {
    let (ta, tb) = (compute(first), compute(second));
    let provisional = first
        .victimes
        .iter()
        .flat_map(|v| &v.postes)
        .filter_map(|p| p.montant_alloue.filter(|m| *m > 0.0))
        .any(|m| text.provisional(m));
    let decision = if unsure_base(first) || unsure_base(second) {
        Some(UNSURE_BASE)
    } else if text.aggravation {
        Some(AGGRAVATION)
    } else if provisional {
        Some(PROVISION)
    } else {
        None
    };
    let (disputed_a, disputed_b) = (contested(&ta), contested(&tb));
    let mut other: HashMap<Fact, usize> = HashMap::new();
    for f in observations(second, &tb)
        .iter()
        .filter(|o| on_table(o) && !disputed_b.contains(&(o.victime_ord, o.poste_ord)))
        .filter_map(fact)
    {
        *other.entry(f).or_default() += 1;
    }
    let rate = euro_rate(first.devise.as_deref());
    let mut rows = observations(first, &ta);
    let mourned = |o: &Observation| o.axe.as_deref() == Some("categorie") && o.deces == Some(true);
    let uncaused = text.uncaused_death
        || rows.iter().filter(|o| on_table(o) && mourned(o)).any(|o| {
            first.victimes[o.victime_ord as usize].postes[o.poste_ord as usize]
                .montant_alloue
                .is_some_and(|m| text.survival(m))
        });
    for o in rows.iter_mut().filter(|o| on_table(o)) {
        let v = &first.victimes[o.victime_ord as usize];
        let p = &v.postes[o.poste_ord as usize];
        let amount = p
            .montant_alloue
            .filter(|_| o.axe.as_deref() != Some("unite"));
        let reason = decision
            .or_else(|| {
                disputed_a
                    .contains(&(o.victime_ord, o.poste_ord))
                    .then_some(CONTESTED)
            })
            .or_else(|| {
                let f = fact(o).expect("a row on the table has an axis");
                match other.get_mut(&f) {
                    Some(n) if *n > 0 => {
                        *n -= 1;
                        None
                    }
                    _ => Some(ONE_READING),
                }
            })
            .or_else(|| o.indemnites_acquises.is_some().then_some(ACQUIRED))
            .or_else(|| {
                let rate = rate?;
                [o.montant, o.valeur_unitaire]
                    .into_iter()
                    .flatten()
                    .any(|x| text.capped(x / rate))
                    .then_some(CAPPED)
            })
            .or_else(|| {
                let m = amount?;
                let points = o.quantite.filter(|_| o.axe.as_deref() == Some("grille"));
                (text.share_only(m, o.cotation.or(points))
                    || o.cotation.is_some_and(|c| text.split_note(c)))
                .then_some(SHARE)
            })
            .or_else(|| {
                let price = o
                    .valeur_unitaire
                    .filter(|_| o.axe.as_deref() == Some("unite"))?;
                text.shared_price(price / rate?).then_some(SHARED_TARIFF)
            })
            .or_else(|| {
                let price = o
                    .valeur_unitaire
                    .filter(|_| o.axe.as_deref() == Some("unite"))?;
                text.residual_price(price / rate?, o.unite.as_deref()?)
                    .then_some(RESIDUAL_TARIFF)
            })
            .or_else(|| {
                let twin = second
                    .victimes
                    .iter()
                    .filter(|w| w.cle == v.cle)
                    .flat_map(|w| &w.postes)
                    .find(|q| q.poste == p.poste);
                let labelled = [Some(p), twin]
                    .into_iter()
                    .flatten()
                    .filter_map(|q| q.libelle_decision.as_deref())
                    .any(|l| GLOBAL_WORD.is_match(&lj_fold::fold(l)));
                amount
                    .is_some_and(|m| labelled || text.global(m))
                    .then_some(GLOBAL)
            })
            .or_else(|| (amount.is_some() && joint(first, v, p)).then_some(JOINT))
            .or_else(|| {
                let m = amount.filter(|_| o.axe.as_deref() == Some("categorie"))?;
                text.several_dead(m).then_some(SEVERAL_DEATHS)
            })
            .or_else(|| (uncaused && amount.is_some() && mourned(&*o)).then_some(UNCAUSED_DEATH))
            .or_else(|| {
                // The temporary esthetic damage is the one experts rate period by period.
                let cotation = o.cotation.filter(|_| o.poste == "PET");
                let (Some(m), Some(c)) = (amount, cotation) else {
                    return None;
                };
                text.cotation_changes(m, c).then_some(PERIOD_COTATION)
            })
            .or_else(|| {
                (o.axe.as_deref() == Some("grille") && o.taux_pct.is_some()).then_some(PRORATED_DFP)
            })
            .or_else(|| {
                let (Some(price), Some(rate)) = (o.valeur_unitaire, rate) else {
                    return None;
                };
                let days = match o.unite.as_deref() {
                    Some("jour") => 1.0,
                    Some("semaine") => 7.0,
                    Some("mois") => 365.25 / 12.0,
                    _ => return None,
                };
                (o.poste == "DFT"
                    && o.axe.as_deref() == Some("unite")
                    && o.taux_pct.is_none()
                    && (price / days < LOWEST_FULL_DFT || text.partial_rate(price / rate)))
                .then_some(PRORATED_DFT)
            })
            .or_else(|| {
                let m = o.montant.filter(|_| o.axe.as_deref() != Some("unite"))?;
                (!text.writes(m / rate?)).then_some(UNWRITTEN_AMOUNT)
            })
            .or_else(|| {
                let points = o.quantite.filter(|_| o.axe.as_deref() == Some("grille"))?;
                (!text.percent(points)).then_some(UNWRITTEN_RATE)
            })
            .or_else(|| {
                let age = o.age.filter(|_| o.axe.as_deref() == Some("grille"))?;
                (!text.age(age)).then_some(UNWRITTEN_AGE)
            })
            .or_else(|| {
                let (Some(price), Some(unite), Some(rate)) =
                    (o.valeur_unitaire, o.unite.as_deref(), rate)
                else {
                    return None;
                };
                (o.axe.as_deref() == Some("unite") && !text.tariff(price / rate, unite))
                    .then_some(UNWRITTEN_TARIFF)
            });
        o.hors_table = reason.map(str::to_string);
        if let (None, Some("grille"), Some(point), Some(rate)) =
            (reason, o.axe.as_deref(), o.valeur_unitaire, rate)
        {
            if !text.writes(point / rate) {
                o.valeur_unitaire = None;
            }
        }
        if let (Some(nature), Some(price), Some(rate), Some(c)) = (
            o.nature_aide.clone(),
            o.valeur_unitaire,
            rate,
            o.comparabilite.as_mut(),
        ) {
            c.nature_aide_ecrite = text.names_nature(price / rate, &nature);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::super::{NatureAide, Unite};
    use super::*;

    fn tariff(text: &str, price: f64, unite: &str) -> bool {
        Written::new(text).tariff(price, unite)
    }

    #[test]
    fn a_tariff_reads_as_price_currency_then_unit() {
        let t = "une aide humaine de 3 heures par jour au taux de 19 euros de l'heure";
        assert!(tariff(t, 19.0, "heure"));
        assert!(!tariff(t, 19.0, "jour"));
        assert!(tariff("sur la base de 873 € par mois", 873.0, "mois"));
        assert!(tariff("soit 1 800,00 euros/mois", 1800.0, "mois"));
        assert!(tariff("à raison de 164,52 euros par jour", 164.52, "jour"));
    }

    #[test]
    fn a_unit_named_before_the_price_is_an_adjective_of_it() {
        assert!(tariff("au taux horaire de 20 euros", 20.0, "heure"));
        assert!(tariff("sur une base journalière de 25 €", 25.0, "jour"));
        assert!(tariff(
            "une dépense annuelle moyenne de 1 140 euros",
            1140.0,
            "euro_de_rente"
        ));
        assert!(!tariff(
            "3 heures par jour au taux de 19 euros",
            19.0,
            "jour"
        ));
        assert!(!tariff("un taux horaire de 20 heures", 20.0, "heure"));
    }

    #[test]
    fn an_age_is_read_where_the_text_writes_it_in_years() {
        let t = Written::new("âgée de 41 ans à la faute et de 43 ans à la consolidation");
        assert!(t.age(43.0));
        assert!(!t.age(45.0));
        assert!(!Written::new("né le 12 mars 1983, consolidé le 30 novembre 2018").age(35.0));
    }

    #[test]
    fn a_dfp_rate_is_read_where_the_text_writes_it_as_a_rate() {
        let t = Written::new(
            "un déficit évalué à 60 % pour les troubles cognitifs et à 5% pour le déficit moteur, \
             un taux de 12,5 pour cent, une incapacité de 10 points, un déficit permanent : 9'%, \
             65 000 euros",
        );
        assert!(t.percent(60.0) && t.percent(5.0) && t.percent(12.5) && t.percent(10.0));
        assert!(t.percent(9.0) && !t.percent(65.0));
    }

    #[test]
    fn a_number_without_a_currency_is_no_tariff() {
        assert!(!tariff("du 20 avril au 3 mai, soit 14 jours", 20.0, "jour"));
        assert!(!tariff("un taux de 25 % par jour", 25.0, "jour"));
    }

    #[test]
    fn a_unit_out_of_reach_or_before_the_price_does_not_count() {
        assert!(!tariff(
            "la somme de 14 euros, compte tenu des frais engagés, par semaine",
            14.0,
            "semaine"
        ));
        assert!(!tariff("par jour : 25 euros", 25.0, "jour"));
    }

    #[test]
    fn an_annuity_reads_on_its_annual_base() {
        assert!(tariff(
            "une perte de 12 000 euros par an capitalisée",
            12000.0,
            "euro_de_rente"
        ));
    }

    #[test]
    fn an_aggravation_is_what_the_court_repairs_not_a_risk_it_reserves() {
        let yes = [
            "en indemnisation de leurs prejudices resultant de cette aggravation",
            "fixe le montant de l'indemnisation de l'aggravation des prejudices",
            "dfp au titre de l'aggravation : 3 %",
            "la date de consolidation post-aggravation",
            "taux global d'aipp : 16% dont 1% d'aggravation",
            "un deficit fonctionnel propre a l'aggravation du prejudice initial",
            "date retenue comme point de depart de l'aggravation : 25 juillet 2016",
            "relevant par la suite une nouvelle aggravation de son etat",
            "une consolidation de cette aggravation au 29/01/2019",
        ];
        for t in yes {
            assert!(repairs_aggravation(t), "{t}");
        }
        let no = [
            "sous reserve d'aggravation",
            "le risque d'aggravation justifie une reserve",
            "une aggravation de l'etat anterieur n'est pas demontree",
        ];
        for t in no {
            assert!(!repairs_aggravation(t), "{t}");
        }
    }

    #[test]
    fn a_provision_is_an_amount_written_only_as_an_advance() {
        let t = Written::new(
            "6. Mme A est en droit d'obtenir, de manière non sérieusement contestable, une \
             indemnité provisionnelle de 5 600 euros en réparation du déficit fonctionnel \
             permanent, outre une somme de 2 000 euros au titre des souffrances endurées.",
        );
        assert!(t.provisional(5600.0) && t.provisional(2000.0));
        let t = Written::new(
            "Il lui est alloué 10 000 euros, sous déduction de la provision de 3 000 euros.",
        );
        assert!(!t.provisional(10000.0));
        let t = Written::new(
            "Une provision de 4 000 euros a été versée. Le tribunal fixe le préjudice à 4 000 euros.",
        );
        assert!(!t.provisional(4000.0));
    }

    #[test]
    fn an_amount_the_claim_caps_is_not_the_courts_own() {
        let t = Written::new(
            "Son préjudice sera justement évalué, en tenant compte du quantum de la demande, à \
             1 400 euros du point, soit 5 600 euros. La cour ne pouvant statuer ultra petita, elle \
             confirme l'indemnisation au titre du DFP à hauteur de 63.000 euros. La victime ne \
             réclamant que 9.997 € à ce titre, le calcul se fera sur cette base.",
        );
        assert!(t.capped(1400.0) && t.capped(5600.0) && t.capped(63000.0) && t.capped(9997.0));
        let t = Written::new(
            "M. B limite sa demande à 5 000 euros. Le préjudice est évalué à 3 000 euros. La cour \
             ne peut statuer ultra petita sur les intérêts.",
        );
        assert!(!t.capped(5000.0) && !t.capped(3000.0));
        let t = Written::new(
            "Il sollicite 1h/j x 20€/h = 7280€/an. Le tribunal étant tenu par les prétentions des \
             parties, il sera fait droit à la demande. L'indemnité sollicitée à hauteur de 50 € \
             n'apparaît pas excessive.",
        );
        assert!(t.capped(7280.0) && !t.capped(50.0));
    }

    #[test]
    fn a_class_rate_is_not_the_full_daily_tariff() {
        let t = Written::new(
            "Le déficit est indemnisé sur la base 25 euros par jour pour un déficit total soit :\n\
             - déficit fonctionnel temporaire de classe 3 : 12,50 € par jour ;\n\
             - déficit fonctionnel temporaire de classe 2 : 6,25 € par jour ;\n\
             Du 16 au 28 mars 2019 : 13 jours à 6,25 € = 81,25 €. La base varie entre 18 et 33 \
             euros par jour pour un taux de 100% soit une somme de 8,25 euros par jour pour un \
             taux de 25%.",
        );
        assert!(t.partial_rate(12.5) && t.partial_rate(6.25) && t.partial_rate(8.25));
        assert!(!t.partial_rate(25.0) && !t.partial_rate(33.0));
        let t = Written::new(
            "(6 jours x 30 euros) + (489 jours x 30 euros x 25 %) = 3 847,50 euros. Il demande 301 \
             jours de DFTP, à 50% à 28 euros par jour.",
        );
        assert!(!t.partial_rate(30.0) && !t.partial_rate(28.0));
    }

    #[test]
    fn an_amount_for_a_share_of_the_damage_does_not_price_the_whole_note() {
        let t = Written::new(
            "Les souffrances endurées auraient été de 3 sur une échelle de 1 à 7 en l'absence \
             d'infection. L'expert les évalue à 4 sur une échelle de 1 à 7. Il sera fait une juste \
             appréciation des souffrances en lien avec les seules conséquences de l'infection, en \
             lui allouant la somme de 1 000 euros.",
        );
        assert!(t.share_only(1000.0, Some(4.0)));
        let t = Written::new(
            "Le déficit fonctionnel permanent a été évalué à 12% dont 8% liés aux conséquences de \
             l'infection. Il sera fait une juste appréciation de ce préjudice en l'évaluant à 15 000 \
             euros. De surcroît, les souffrances cotées 4/7 justifient 8 000 euros.",
        );
        assert!(t.share_only(15000.0, Some(12.0)) && !t.share_only(15000.0, Some(8.0)));
        assert!(!t.share_only(8000.0, Some(4.0)));
        let t = Written::new(
            "L'expert a évalué les souffrances à 6 sur une échelle de 1 à 7 dont 3 sur 7 relèvent \
             du retard ; des souffrances endurées de 4/7 (5/7 au total dont 1/7 au titre d'une \
             autre pathologie).",
        );
        assert!(t.split_note(6.0) && t.split_note(5.0));
        assert!(!t.split_note(3.0) && !t.split_note(4.0));
    }

    #[test]
    fn an_amount_for_what_the_facts_added_to_a_prior_state_is_a_share() {
        let t = Written::new(
            "Les souffrances endurées sont évaluées par l'expert à 5 sur une échelle de 7. Il résulte \
             toutefois de l'instruction qu'une partie des souffrances, notamment sa fatigabilité, \
             préexistait à la maladie professionnelle en cause. Il sera fait une juste appréciation \
             de ce chef de préjudice en lui allouant une somme de 4 000 euros.",
        );
        assert!(t.share_only(4000.0, Some(5.0)));
        let t = Written::new(
            "Son déficit fonctionnel permanent est estimé à 50 % par l'expert, soit un taux identique \
             au taux d'incapacité qui lui avait été attribué en 2006. Il sera fait une juste \
             évaluation de ce préjudice en le fixant à la somme de 5 000 euros.",
        );
        assert!(t.share_only(5000.0, Some(50.0)));
        let t = Written::new(
            "L'expert ne retient aucun état antérieur préexistant. Les souffrances endurées, cotées \
             3/7, justifient une somme de 6 000 euros.",
        );
        assert!(!t.share_only(6000.0, Some(3.0)));
    }

    #[test]
    fn a_price_written_after_its_share_is_not_the_full_one() {
        let t = Written::new(
            "Soit une perte annuelle de 7 475,95 euros dont 80 % imputable aux séquelles = 5 980,76 \
             euros. Il convient de capitaliser la somme annuelle de 5 980,76 euros par le coefficient \
             24,677. Les frais ne sont liés qu'à moitié à l'infection et indemnisés pour moitié ; en \
             appliquant un tarif horaire de 20 euros, l'indemnité est de 1 200 euros.",
        );
        assert!(t.shared_price(5980.76));
        assert!(!t.shared_price(7475.95) && !t.shared_price(20.0));
    }

    #[test]
    fn a_price_a_payer_leaves_to_the_victim_is_not_the_full_one() {
        let t = Written::new(
            "200 séances de rééducation seront indemnisées à hauteur de 1000 € compte tenu de la somme \
             de 5€ par séance restant à la charge de cette dernière. Au prix de 45 € la séance, dont \
             25 € pris en charge, 20 € par séance restent à sa charge. L'aide est indemnisée au taux \
             horaire de 18 euros. Les frais restés à sa charge : 18 euros.",
        );
        assert!(t.residual_price(5.0, "seance") && t.residual_price(20.0, "seance"));
        assert!(!t.residual_price(45.0, "seance") && !t.residual_price(18.0, "heure"));
    }

    #[test]
    fn one_sum_for_several_deaths_is_no_relatives_own() {
        let t = Written::new(
            "Il sera fait une juste appréciation du préjudice d'affection subi par M. B du fait du \
             décès de ses deux enfants en l'évaluant à la somme globale de 50 000 euros. Sa mère, \
             du fait du décès de son fils, recevra 25 000 euros.",
        );
        assert!(t.several_dead(50000.0) && !t.several_dead(25000.0));
    }

    #[test]
    fn a_death_the_text_does_not_lay_on_the_facts_bereaves_no_relative() {
        for text in [
            "M. N est décédé le 3 mai 2019 d'une pathologie sans lien avec l'accident.",
            "L'assureur rappelle que le décès de M. N est sans lien avec l'accident.",
            "Elle est décédée de causes étrangères à l'intervention.",
        ] {
            assert!(Written::new(text).uncaused_death, "{text}");
        }
        for text in [
            "L'assistance que fournissait l'épouse avant son décès est sans lien avec l'infection.",
            "M. N est décédé des suites de l'accident.",
        ] {
            assert!(!Written::new(text).uncaused_death, "{text}");
        }
        let t = Written::new(
            "Compte tenu des souffrances endurées par M. I et de la dégradation de son état de \
             santé, le préjudice d'affection de son épouse est évalué à la somme de 1 000 euros. \
             Le décès de leur fils leur a causé un préjudice d'affection évalué à 25 000 euros.",
        );
        assert!(t.survival(1000.0) && !t.survival(25000.0));
    }

    #[test]
    fn a_global_amount_prices_several_postes() {
        let t = Written::new(
            "Les préjudices esthétiques temporaire et permanent, tous deux évalués à 4/7, doivent \
             faire l'objet d'une évaluation globale et justifient une indemnité de 13 000 euros.",
        );
        assert!(t.global(13000.0));
        let t = Written::new(
            "Ces éléments justifient que lui soit accordée une indemnisation globale pour les \
             souffrances endurées de 8 000 euros.",
        );
        assert!(!t.global(8000.0));
    }

    #[test]
    fn the_nature_of_an_aid_is_written_only_where_the_court_names_it() {
        let t = Written::new(
            "Il a besoin d'une aide humaine active pour la toilette et l'habillage. Elle sera \
             indemnisée au taux horaire de 20 euros.",
        );
        assert_eq!(t.names_nature(20.0, "active"), Some(true));
        let t = Written::new(
            "Il a besoin d'une aide pour la toilette et l'habillage, non spécialisée. Elle sera \
             indemnisée au taux horaire de 20 euros.",
        );
        assert_eq!(t.names_nature(20.0, "active"), Some(false));
        assert_eq!(t.names_nature(20.0, "autre"), None);
    }

    #[test]
    fn a_sum_labelled_for_several_relatives_is_joint_unless_each_has_a_victim() {
        let parent = |libelle: &str| Victime {
            role: "indirecte".into(),
            lien: Some(super::super::Lien::Parent),
            postes: vec![Poste {
                poste: Some("PAF".into()),
                libelle_decision: Some(libelle.into()),
                montant_alloue: Some(30000.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        let q = |victimes: Vec<Victime>| Quantum {
            victimes,
            ..Default::default()
        };
        let one = q(vec![parent("préjudice d'affection des parents")]);
        assert!(joint(&one, &one.victimes[0], &one.victimes[0].postes[0]));
        let two = q(vec![
            parent("préjudice moral des parents"),
            parent("préjudice moral des parents"),
        ]);
        assert!(!joint(&two, &two.victimes[0], &two.victimes[0].postes[0]));
        let each = q(vec![parent(
            "préjudice d'affection des parents, pour chacun",
        )]);
        assert!(!joint(
            &each,
            &each.victimes[0],
            &each.victimes[0].postes[0]
        ));
    }

    #[test]
    fn a_cotation_that_changes_with_the_period_is_not_the_amounts_own() {
        let t = Written::new(
            "Il lui sera alloué au titre des souffrances endurées la somme de 8000 €.\n\nL'expert \
             retient un préjudice esthétique temporaire de 3/7 jusqu'au 31 mai 2022. A compter du \
             1er juin 2022 le dommage esthétique temporaire est constitué par les cicatrices, il est \
             équivalent au dommage esthétique définitif de 2/7.\n\nIl sera alloué à Mme F la somme \
             de 3000€.",
        );
        assert!(t.cotation_changes(3000.0, 3.0));
        let t = Written::new(
            "L'expert fixe le préjudice esthétique temporaire à 3/7 du 26 novembre 2013 au 31 mars \
             2014, 2/7 du 1er avril 2014 au 30 septembre 2014. Il est alloué 2 500 euros.",
        );
        assert!(t.cotation_changes(2500.0, 3.0));
        let t = Written::new(
            "L'expert retient un préjudice esthétique temporaire, évalué à 3/7, durant les 3 \
             premières semaines.\n\nLe préjudice esthétique définitif est évalué à 1,5/7.\n\nIl y a \
             lieu d'évaluer le préjudice temporaire à la somme de 1 500 euros.",
        );
        assert!(!t.cotation_changes(1500.0, 3.0));
        let t = Written::new(
            "- préjudice esthétique temporaire : 2/7 du 1er au 30 juin 2020\n- déficit fonctionnel \
             permanent : 5 % à compter de la consolidation\n- préjudice esthétique permanent : 1/7\n\
             Il est alloué 1 000 euros au titre du préjudice esthétique temporaire.",
        );
        assert!(!t.cotation_changes(1000.0, 2.0));
    }

    fn reading(montant: f64, prix: f64, partage: f64) -> Quantum {
        Quantum {
            est_liquidation: true,
            victimes: vec![Victime {
                cle: "v1".into(),
                role: "directe".into(),
                partage_responsabilite_pct: Some(partage),
                postes: vec![Poste {
                    poste: Some("ATP".into()),
                    montant_alloue: Some(montant),
                    valeur_unitaire: Some(prix),
                    quantite: Some(100.0),
                    unite: Some(Unite::Heure),
                    nature_aide: Some(NatureAide::Active),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn kept(first: &Quantum, second: &Quantum, text: &str) -> Vec<Option<String>> {
        publish(first, second, &Written::new(text))
            .into_iter()
            .map(|o| o.hors_table)
            .collect()
    }

    #[test]
    fn only_what_both_readings_publish_identically_is_kept() {
        let text = "une aide de 100 heures à 19 euros de l'heure, soit 1 900 euros";
        let a = reading(1900.0, 19.0, 100.0);
        assert_eq!(kept(&a, &a, text), vec![None]);
        let b = reading(2400.0, 24.0, 100.0);
        assert_eq!(kept(&a, &b, text), vec![Some(ONE_READING.to_string())]);
    }

    #[test]
    fn a_tariff_both_readings_agree_on_still_needs_its_passage() {
        let text = "une aide de 100 heures, soit 1 900 euros";
        let a = reading(1900.0, 19.0, 100.0);
        assert_eq!(kept(&a, &a, text), vec![Some(UNWRITTEN_TARIFF.to_string())]);
    }

    #[test]
    fn a_poste_its_own_arithmetic_disputes_publishes_nothing() {
        let text = "une aide de 100 heures à 19 euros de l'heure, soit 2 000 euros";
        let a = reading(2000.0, 19.0, 100.0);
        assert_eq!(kept(&a, &a, text), vec![Some(CONTESTED.to_string())]);
    }

    #[test]
    fn a_prorated_dfp_gives_no_point_value() {
        let dfp = |taux_pct| Quantum {
            est_liquidation: true,
            victimes: vec![Victime {
                cle: "v1".into(),
                role: "directe".into(),
                partage_responsabilite_pct: Some(100.0),
                taux_dfp_pct: Some(10.0),
                postes: vec![Poste {
                    poste: Some("DFP".into()),
                    montant_alloue: Some(16000.0),
                    valeur_unitaire: Some(2000.0),
                    taux_pct,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let text = "2 000 euros le point, soit 16 000 euros après une perte de chance de 80 %";
        assert_eq!(
            kept(&dfp(Some(80.0)), &dfp(Some(80.0)), text),
            vec![Some(PRORATED_DFP.to_string())]
        );
    }

    #[test]
    fn a_full_amount_the_text_never_writes_is_off_the_table() {
        let dfp = || Quantum {
            est_liquidation: true,
            victimes: vec![Victime {
                cle: "v1".into(),
                role: "directe".into(),
                partage_responsabilite_pct: Some(50.0),
                taux_dfp_pct: Some(13.0),
                postes: vec![Poste {
                    poste: Some("DFP".into()),
                    montant_alloue: Some(29900.0),
                    montant_du: Some(14950.0),
                    valeur_unitaire: Some(2300.0),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let text = "il lui sera alloué une indemnité de 14.950 € (13 x 2.300 x 50%)";
        assert_eq!(
            kept(&dfp(), &dfp(), text),
            vec![Some(UNWRITTEN_AMOUNT.to_string())]
        );
        let text =
            "le déficit est évalué à 29 900 euros (13 x 2.300 €), soit 14.950 € après partage";
        assert_eq!(kept(&dfp(), &dfp(), text), vec![None]);
    }

    #[test]
    fn a_split_without_a_reduced_amount_keeps_nothing() {
        let text = "une aide de 100 heures à 19 euros de l'heure, soit 1 900 euros, dont la moitié, 950 euros";
        let full = reading(1900.0, 19.0, 100.0);
        let mut split = reading(1900.0, 19.0, 50.0);
        assert_eq!(
            kept(&full, &split, text),
            vec![Some(UNSURE_BASE.to_string())]
        );
        split.victimes[0].postes[0].montant_du = Some(950.0);
        assert_eq!(kept(&full, &split, text), vec![None]);
    }

    #[test]
    fn a_dfp_row_takes_its_rate_and_its_point_value_from_the_text() {
        let dfp = |taux: f64, montant: f64| Quantum {
            est_liquidation: true,
            victimes: vec![Victime {
                cle: "v1".into(),
                role: "directe".into(),
                partage_responsabilite_pct: Some(100.0),
                taux_dfp_pct: Some(taux),
                postes: vec![Poste {
                    poste: Some("DFP".into()),
                    montant_alloue: Some(montant),
                    valeur_unitaire: Some(montant / taux),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let q = dfp(4.0, 7080.0);
        let text = "un déficit fonctionnel permanent de 4 %, il lui sera alloué 7.080 €";
        let row = &publish(&q, &q, &Written::new(text))[0];
        assert_eq!(
            (row.hors_table.as_deref(), row.valeur_unitaire),
            (None, None)
        );
        let text = "un déficit fonctionnel permanent de 4 %, soit 1.770 € le point et 7.080 €";
        assert_eq!(
            publish(&q, &q, &Written::new(text))[0].valeur_unitaire,
            Some(1770.0)
        );
        let q = dfp(65.0, 65000.0);
        let text = "un déficit évalué à 60 % et à 5 %, réparé par 65 000 euros";
        assert_eq!(kept(&q, &q, text), vec![Some(UNWRITTEN_RATE.to_string())]);
    }
}
