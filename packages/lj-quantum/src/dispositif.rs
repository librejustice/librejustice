//! Reading of the dispositif in two modes. Sure: a poste takes an amount only
//! through the written link « X au titre de P » or « P : X », a victim is a
//! named natural person, and every other amount is an obligation with its best
//! cause. Extended: the allocation verbs, the reduction read on the clause that
//! applies it, the confirmed amounts, the table columns, the heirs of a named
//! deceased, and the postes the motifs price under the court's own award verb.
use super::leftovers::{self, Amount};
use super::vocabulary::{self, Cue, CueTok};
use super::{Composante, Lecture, Lien, Poste, Quantum, Rente, Victime};
use lj_extract::scan::DocScan;

/// `Sure` writes only what the court fixes itself; `Extended` adds the readings
/// a finer proof carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sure,
    Extended,
}

impl Mode {
    fn extended(self) -> bool {
        self == Mode::Extended
    }

    fn agent(self) -> &'static str {
        match self {
            Mode::Sure => "extracteur-sur",
            Mode::Extended => "extracteur-etendu",
        }
    }
}

/// What a final record owes to an amount the dispositif writes.
#[derive(Clone, Debug, PartialEq)]
pub enum Nature {
    /// An amount of a poste of a victim, in whichever field the record settles.
    Poste { cle: Option<String>, code: String },
    /// An indemnity paid to a victim whose poste the text does not name.
    Indemnite { cle: Option<String> },
    /// What a victim receives in all.
    Net { cle: String },
    /// A total the court writes.
    Total { cle: Option<String> },
    /// Not an indemnity of a victim, with its cause.
    HorsRecord(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Obligation {
    pub montant: f64,
    pub s: usize,
    pub e: usize,
    pub nature: Nature,
}

#[derive(Debug, Default)]
pub struct Extraction {
    pub quantum: Quantum,
    pub obligations: Vec<Obligation>,
}

#[derive(Clone, Copy)]
enum Event {
    Amount(Amount),
    Cue(CueTok),
}

impl Event {
    fn pos(&self) -> usize {
        match self {
            Event::Amount(a) => a.start,
            Event::Cue(c) => c.s,
        }
    }

    fn end(&self) -> usize {
        match self {
            Event::Amount(a) => a.end,
            Event::Cue(c) => c.e,
        }
    }

    fn cue(&self) -> Option<Cue> {
        match self {
            Event::Cue(c) => Some(c.cue),
            Event::Amount(_) => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Condemn,
    Fix,
    Reject,
    Recital,
    Other,
}

const RECITAL_HEADS: [&str; 8] = [
    "confirme", "infirme", "reforme", "annule", "casse", "sauf", "emendant", "emende",
];
const RECITAL_BEFORE: [&str; 4] = ["qu'il", "qu'elle", "qui a", "qui ont"];
/// Verbs that award without naming a payee after « à payer à ».
const ALLOCATES: [&str; 4] = ["alloue", "accorde", "attribue", "octroie"];
/// Where a confirmation stops keeping the judgment's chiefs.
const EXCEPTIONS: [&str; 7] = [
    "sauf",
    "excepte",
    "a l'exception",
    "hormis",
    "infirme",
    "reforme",
    "emend",
];
/// The exception words that can stand alone at the end of a line, the chiefs
/// they carve out following on the next ones.
const OPEN_EXCEPTIONS: [&str; 4] = ["sauf", "excepte", "a l'exception", "hormis"];
const CIVILITIES: [&str; 11] = [
    "messieurs ",
    "mesdames ",
    "monsieur ",
    "madame ",
    "mademoiselle ",
    "mmes ",
    "mme ",
    "mlle ",
    "mm. ",
    "m. ",
    "m ",
];
const EMPTY_ROW: [&str; 6] = [
    "neant",
    "rejet",
    "sans objet",
    "deboute",
    "aucun",
    "non justifie",
];
const NETTING: [&str; 6] = [
    "imputation",
    "deduction",
    "deduire",
    "deduit",
    "tenu compte",
    "compte tenu",
];
const NETTED: [&str; 7] = [
    "deja versee",
    "deja allouee",
    "deja percue",
    "non deduite",
    "versees",
    "a deduire",
    "a imputer",
];
/// « ès qualités de tuteur de M. Y » : the money is the ward's.
const WARDENS: [&str; 11] = [
    "tuteur",
    "tutrice",
    "curateur",
    "curatrice",
    "administrateur legal",
    "administratrice legale",
    "administrateurs legaux",
    "representant legal",
    "representante legale",
    "representants legaux",
    "representantes legales",
];
const KIN: [&str; 12] = [
    "leur ",
    "leurs ",
    "son ",
    "sa ",
    "l'enfant ",
    "enfant ",
    "fils ",
    "fille ",
    "le mineur ",
    "la mineure ",
    "mineur ",
    "mineure ",
];
const REPRESENTING: [&str; 5] = [
    "qualite",
    "agissant",
    "au nom de",
    "pour le compte de",
    "representant legal",
];
const NAME_STOPS: [&str; 25] = [
    ",",
    ":",
    ";",
    " et ses suites",
    " la somme",
    " les sommes",
    " une somme",
    " celle de",
    " en deniers",
    " a titre",
    " au titre",
    " en reparation",
    " sur le fondement",
    " le montant",
    " les montants",
    " comme suit",
    " ainsi",
    " consecutif",
    " resultant",
    " suite",
    " du fait",
    " victime",
    " de la maniere",
    " represente",
    " assiste",
];
/// Where the ruling that carried the old figure is named, right behind the
/// payee the appeal rewrites the sum of.
const BY_JUDGMENT: [&str; 5] = [
    "par le jugement",
    "par l'arret",
    "par l'article",
    "par le tribunal",
    "par la cour",
];

/// How the administrative appeal rewrites the figure of the judgment rather
/// than condemning again: what follows is the sum that replaces it.
const RAISED: [&str; 6] = [
    "est portee a ",
    "est porte a ",
    "sont portees a ",
    "est ramenee a ",
    "est ramene a ",
    "sont ramenees a ",
];

/// After a recital, the court rules for itself: a condemnation past these words
/// is its own.
const RENEWED: [&str; 4] = [
    "statuant a nouveau",
    "statuant de nouveau",
    "y ajoutant",
    "ajoutant au jugement",
];
/// A clause that lists what follows.
const LISTING: [&str; 4] = [
    "de la facon suivante",
    "comme suit",
    "ainsi qu'il suit",
    "de la maniere suivante",
];
/// A clause that speaks of the judgment under appeal.
const RECITED: [&str; 4] = ["jugement", "tribunal", "premiers juges", "ordonnance"];
/// A payee the clause named before: « ce dernier », « celle-ci ».
const ANAPHORA: [&str; 4] = ["ce dernier", "cette derniere", "celui-ci", "celle-ci"];
/// What « et » joins to a payee when it opens another one.
const JOINED: [&str; 10] = [
    "les ", "son ", "sa ", "ses ", "leur", "l'", "la ", "le ", "consorts", "societe",
];
const RICOCHET_WORDS: [&str; 14] = [
    "ricochet",
    "les parents",
    "des parents",
    "aux parents",
    "ayant droit",
    "ayants droit",
    "victime indirecte",
    "victimes indirectes",
    "en sa qualite de",
    "ses enfants",
    "ses parents",
    "sa mere",
    "son pere",
    "ses proches",
];
/// The kinship an apposition names beside the payee, longest surface first so
/// that « petite-fille » is not read as « fille ».
#[rustfmt::skip]
const KINSHIP: [(&str, Lien); 38] = [
    ("grands-parents", Lien::GrandParent), ("grand-parent", Lien::GrandParent),
    ("grand-mere", Lien::GrandParent), ("grand-pere", Lien::GrandParent),
    ("petits-enfants", Lien::PetitEnfant), ("petit-enfant", Lien::PetitEnfant),
    ("petite-fille", Lien::PetitEnfant), ("petit-fils", Lien::PetitEnfant),
    ("belle-mere", Lien::BeauParent), ("beau-pere", Lien::BeauParent),
    ("belle-fille", Lien::BeauFils), ("beau-fils", Lien::BeauFils),
    ("beau-frere", Lien::Fratrie), ("belle-soeur", Lien::Fratrie),
    ("gendre", Lien::BeauFils), ("bru", Lien::BeauFils),
    ("veuve", Lien::Conjoint), ("veuf", Lien::Conjoint), ("epouse", Lien::Conjoint),
    ("epoux", Lien::Conjoint), ("conjointe", Lien::Conjoint), ("conjoint", Lien::Conjoint),
    ("concubine", Lien::Concubin), ("concubin", Lien::Concubin),
    ("compagne", Lien::Concubin), ("compagnon", Lien::Concubin),
    ("partenaire", Lien::Partenaire), ("pacse", Lien::Partenaire), ("pacsee", Lien::Partenaire),
    ("fille", Lien::Enfant), ("fils", Lien::Enfant), ("enfant", Lien::Enfant),
    ("mere", Lien::Parent), ("pere", Lien::Parent),
    ("soeur", Lien::Fratrie), ("frere", Lien::Fratrie),
    ("niece", Lien::NeveuNiece), ("neveu", Lien::NeveuNiece),
];

/// What makes a kinship word the other party's, seen from the payee.
const POSSESSIVES: [&str; 5] = ["son ", "sa ", "ses ", "leur ", "leurs "];

/// The kinship an apposition names beside a payee. Two of them in the same
/// breath name two people, and the chef says nothing of the one it pays.
fn kinship_of(raw: &str) -> Option<Lien> {
    // « à payer à [E] [W], représentée par Madame [Y], veuve [W] » : past that
    // word the kinship is the representative's.
    let mut rest = raw.split("represent").next().unwrap_or(raw).to_string();
    // « Madame Michèle B... épouse X... », « [E] épouse [C] » : a married name,
    // which says whose wife she is, not what she is to the victim.
    for w in ["epouse ", "epoux "] {
        while let Some(i) = rest.find(w) {
            let possessive =
                rest[..i].trim_end().ends_with("son") || rest[..i].trim_end().ends_with("sa");
            let after = &rest[i + w.len()..];
            let token: &str = after.split([' ', ',']).next().unwrap_or("");
            let named =
                after.starts_with('[') || token.ends_with("...") || token.chars().count() <= 2;
            if possessive || !named {
                break;
            }
            let cut = i + w.len() + token.len();
            rest.replace_range(i..cut.min(rest.len()), " ");
        }
    }
    let mut found: Option<Lien> = None;
    for (w, l) in KINSHIP {
        let Some(i) = rest.find(w) else {
            continue;
        };
        // « l'accident survenu à ses enfants mineurs » : the word names the
        // other party, and the payee stands on the opposite side of it.
        let owned = POSSESSIVES.iter().any(|p| rest[..i].ends_with(p));
        let l = if owned { l.inverse() } else { l };
        // « petite-fille » holds « fille », which would vote against it: the
        // surface it matched leaves the string with it.
        rest = rest.replace(w, " ");
        match found {
            Some(k) if k != l => return None,
            Some(_) => {}
            None => found = Some(l),
        }
    }
    found
}

/// Heads of damage a fixing clause shares out among named relatives, each of
/// whom takes her own affection.
const AFFECTION_HEADS: [&str; 4] = [
    "prejudices moraux",
    "prejudice moral",
    "prejudices d'affection",
    "prejudice d'affection",
];

/// What names an indirect victim in the text itself. Narrower than
/// `RICOCHET_WORDS`: an heir takes the deceased's own postes, and the record
/// holds them under her, so being one says nothing of the payee's role.
const RICOCHET_NAMED: [&str; 3] = ["ricochet", "victime indirecte", "victimes indirectes"];
/// Wordings by which a group of claimants takes the deceased's own damages.
const SUCCESSORAL: [&str; 8] = [
    "ayants droits",
    "ayants-droits",
    "ayants droit",
    "ayants-droit",
    "ayant droit",
    "heritiers",
    "heritier",
    "succession",
];
/// A payee whose amount is a third-party payer's claim, not a victim's share.
const TIERS_PAYEUR_WORDS: [&str; 8] = [
    "caisse",
    "cpam",
    "c.p.a.m",
    "debours",
    "tiers payeur",
    "prestations servies",
    "recours subrogatoire",
    "organisme social",
];
/// A body whose claim the court fixes or whose recourse it prices.
const ORGANISM_WORDS: [&str; 12] = [
    "cpam",
    "caisse",
    "cps",
    "msa",
    "carsat",
    "cnracl",
    "mutuelle",
    "assurance maladie",
    "rsi",
    "mgen",
    "cramif",
    "tiers payeur",
];
/// Openers of a claim the court fixes for an organism: the whole clause is its.
const CLAIM_OF: [&str; 6] = [
    "la creance de",
    "la creance du",
    "le recours de",
    "les debours de",
    "sa creance",
    "son recours",
];
/// The figure after them is what stays once the provisions are taken off.
const SOLDE_WORDS: [&str; 9] = [
    "total du",
    "somme due",
    "restant du",
    "reste du",
    "solde",
    "reliquat",
    "deduction faite",
    "apres deduction",
    "a deduire",
];
/// How a rente is served, as the record names its period.
const RENTE_PERIODS: [(&str, &str); 6] = [
    ("trimestrielle", "trimestrielle"),
    ("par trimestre", "trimestrielle"),
    ("mensuelle", "mensuelle"),
    ("par mois", "mensuelle"),
    ("annuelle", "annuelle"),
    ("par an", "annuelle"),
];
/// After « à », a body written with a capital opens a payee: « à la CPAM ».
const PAYEE_LEADS: [&str; 4] = ["la ", "l'", "au ", "aux "];
/// A plural payee the clause names without a civility.
const PAYEE_GROUPS: [&str; 4] = ["consorts", "heritiers", "ayants droit", "ayants-droit"];
/// A section of the motifs about the relatives of the person it names.
const RELATIVES_OF: [&str; 14] = [
    "enfants de",
    "enfant de",
    "parents de",
    "epoux de",
    "epouse de",
    "conjoint de",
    "concubin de",
    "concubine de",
    "proches de",
    "ayants droit de",
    "veuve de",
    "veuf de",
    "mere de",
    "pere de",
];
/// Marks that the figure right before them is the victim's share of the poste.
const VICTIM_SHARE_WORDS: [&str; 5] = [
    "en faveur de la victime",
    "au benefice de la victime",
    "revenant a la victime",
    "part de la victime",
    "revenant en definitive",
];
/// Intros saying the list prices what is left after the reduction.
const AFTER_REDUCTION: [&str; 6] = [
    "apres application",
    "apres reduction",
    "apres limitation",
    "apres partage",
    "tenu de la reduction",
    "tenu du partage",
];
const SHARE_WORDS: [&str; 6] = [
    "hors creance",
    "a charge",
    "revenant a",
    "part de la victime",
    "apres deduction",
    "deduction faite",
];
const GLOBAL: [&str; 3] = [
    "prejudice corporel",
    "prejudices corporels",
    "ses prejudices",
];
const GLOBAL_TERMINATED: [&str; 2] = ["son prejudice", "leur prejudice"];
/// Wordings that make the sum the whole of what the victim receives.
const GLOBAL_STRONG: [&str; 7] = [
    "l'ensemble de ses prejudices",
    "l'ensemble de son prejudice",
    "son entier prejudice",
    "l'entier prejudice",
    "l'integralite de son prejudice",
    "toutes causes de prejudice",
    "tous prejudices confondus",
];
const TERMINATORS: [&str; 16] = [
    ",",
    ";",
    ".",
    ")",
    " avec",
    " outre",
    " en deniers",
    " apres",
    " deduction",
    " sous",
    " sauf",
    " et ",
    " a la suite",
    " resultant",
    " consecutif",
    " subi",
];
const CONNECTORS: &[&str] = &[
    "euros",
    "titre",
    "euro",
    "eur",
    "f",
    "cfp",
    "francs",
    "xpf",
    "brut",
    "bruts",
    "net",
    "nets",
    "ttc",
    "ht",
    "hors",
    "taxes",
    "en",
    "capital",
    "deniers",
    "ou",
    "quittances",
    "a",
    "chacun",
    "chacune",
    "principal",
    "l",
    "indemnite",
    "due",
    "fr",
    "fcfp",
    "de",
    "d",
    "et",
    "dommages",
    "interets",
];
/// What opens an item of a list: the figure or the label that follows it.
const BULLETS: [char; 8] = [
    '-', '\u{2013}', '\u{2014}', '\u{2022}', '*', '\u{b0}', '\u{b7}', '>',
];
const ARTICLES: &[&str] = &[
    "le",
    "la",
    "les",
    "l",
    "son",
    "sa",
    "ses",
    "leur",
    "leurs",
    "du",
    "de",
    "des",
    "d",
    "un",
    "une",
    "poste",
    "chef",
    "prejudice",
    "indemnisation",
    "reparation",
    "dispositions",
    "arrerages",
    "echus",
    "echues",
    "echu",
    "echue",
];
const AFTER_LABEL: &[&str] = &[
    "non",
    "deduite",
    "deduites",
    "deduit",
    "deduits",
    "a",
    "la",
    "le",
    "les",
    "l",
    "de",
    "d",
    "du",
    "des",
    "somme",
    "sommes",
    "hauteur",
    "pour",
    "un",
    "une",
    "montant",
    "euros",
    "euro",
    "brut",
    "net",
    "evalue",
    "evaluee",
    "evalues",
    "evaluees",
    "fixe",
    "fixee",
    "fixes",
    "fixees",
    "liquide",
    "liquidee",
    "chiffre",
    "chiffree",
    "arrete",
    "arretee",
    "retenu",
    "retenue",
    "restant",
    "restants",
    "restante",
    "restantes",
    "restee",
    "restees",
    "reste",
    "restes",
    "charge",
    "victime",
    "en",
    "capital",
    "sa",
    "son",
    "hors",
    "creance",
    "creances",
    "tiers",
    "payeurs",
    "part",
    "revenant",
    "apres",
    "deduction",
    "faite",
    "s",
    "elevant",
    "avant",
    "consolidation",
    "temporaire",
    "temporaires",
    "permanent",
    "permanents",
    "permanente",
    "permanentes",
    "futur",
    "futurs",
    "future",
    "futures",
    "echu",
    "echus",
    "echue",
    "echues",
    "echoir",
    "definitif",
    "definitifs",
    "definitive",
    "deja",
    "versee",
    "versees",
    "verse",
    "verses",
    "code",
    "procedure",
    "civile",
    "cpc",
    "dispositions",
];
/// Verbs by which the court itself prices a poste in the motifs.
const AWARD_MARKS: [&str; 24] = [
    "sera alloue",
    "sera allouee",
    "seront alloues",
    "seront allouees",
    "d'allouer",
    "allouer a",
    "sera fixe",
    "sera fixee",
    "de fixer a",
    "fixer a",
    "sera evalue",
    "sera evaluee",
    "l'evaluant a",
    "en l'evaluant",
    "d'evaluer",
    "evalue a",
    "evaluee a",
    "l'arretant a",
    "en l'arretant",
    "sera indemnise",
    "sera indemnisee",
    "sera repare",
    "sera reparee",
    "revient a la victime",
];
/// What makes an award verb the recital of someone else's figure.
const CLAIM_MARKS: [&str; 18] = [
    "demande",
    "demandent",
    "sollicit",
    "reclam",
    "propos",
    "offre",
    "offert",
    "tribunal",
    "premiers juges",
    "jugement",
    "expert",
    "appelant",
    "intime",
    "conclusion",
    "requer",
    "avait alloue",
    "elle evalue",
    "il evalue",
];
/// How far a section of the motifs runs when no other label closes it.
const SECTION: usize = 800;
/// Above this, the section prices intermediate figures and the award is a guess.
const AMOUNTS_PER_SECTION: usize = 2;
/// A rate the court multiplies, not the poste: « 600 euros le point ».
const UNIT_MARKS: [&str; 14] = [
    "le point",
    "du point",
    "par point",
    "par jour",
    "journalier",
    "journaliere",
    "par mois",
    "mensuel",
    "par an",
    "annuel",
    "de rente",
    "par heure",
    "horaire",
    "unitaire",
];
const TEMPORARY: [&str; 3] = [
    "temporaire",
    "avant consolidation",
    "avant la consolidation",
];
const PERMANENT: [&str; 5] = [
    "apres consolidation",
    "apres la consolidation",
    "futur",
    "permanent",
    "definitif",
];

/// A label qualified in the text after it: temporary costs of a permanent poste
/// are frais divers, permanent frais divers are another poste.
fn qualified(code: &'static str, around: &str) -> Option<&'static str> {
    let temporary = TEMPORARY.iter().any(|w| around.contains(w));
    let permanent = PERMANENT.iter().any(|w| around.contains(w));
    match (code, temporary, permanent) {
        ("FVA" | "FLA" | "DSF" | "ATP", true, false) => Some("FD"),
        ("FD", _, true) => None,
        (c, _, _) => Some(c),
    }
}
const BEFORE_LABEL: &[&str] = &[
    "euros", "euro", "pour", "le", "la", "les", "l", "son", "sa", "ses",
];
/// What a row takes off the poste before writing the victim's share.
const DEDUCTED_OF: [&str; 10] = [
    "rente",
    "creance",
    "debours",
    "indemnites journalieres",
    "cpam",
    "caisse",
    "msa",
    "cps",
    "mutuelle",
    "tiers payeur",
];
const AFTER_SUM: &[&str] = &[
    "de",
    "d",
    "i",
    "ii",
    "iii",
    "iv",
    "restant",
    "du",
    "general",
    "generale",
    "un",
    "montant",
    "euros",
    "totale",
    "globale",
    "la",
    "somme",
    "en",
    "principal",
    "capital",
];
/// A list separator between a label and an amount: the amount belongs to the next line.
const SEPARATORS: [&str; 6] = [";", " -", " *", " •", "|", " >"];
/// Between an amount and the label after it, the words that hang the label on
/// that amount: the next amount cannot take it.
const HANGERS: [&str; 5] = [
    "pour",
    "titre",
    "en reparation",
    "en indemnisation",
    "du chef",
];
const REDUCTION_WORDS: [&str; 14] = [
    "droit a indemnisation",
    "droit a reparation",
    "droit a etre indemnise",
    "partage de responsabilite",
    "partage de responsabilites",
    "partage d'imputabilite",
    "part de responsabilite",
    "perte de chance",
    "responsable a hauteur",
    "responsables a hauteur",
    "imputab",
    "faute de la victime",
    "propre faute",
    "sa faute",
];
const REDUCTION_NOISE: [&str; 11] = [
    "dfp",
    "deficit",
    "execution provisoire",
    "interet",
    "tva",
    "horaire",
    "capitalis",
    "100 %",
    "100%",
    "aipp",
    "incapacite",
];
const FRACTIONS: [&str; 6] = [
    "de moitie",
    "la moitie",
    "un tiers",
    "deux tiers",
    "trois quarts",
    "un quart",
];

fn euros(a: &Amount) -> f64 {
    a.value as f64 / 1000.0
}

fn text_of(folded: &[char], s: usize, e: usize) -> String {
    folded[s.min(e)..e].iter().collect()
}

/// Does the text reduce a right to compensation: a percentage or a fraction
/// near a word of reduction, anywhere, since a missed motifs marker would
/// otherwise let a reduced amount pass as the full one.
pub(super) fn reduction_cue(folded: &[char]) -> bool {
    let from = 0usize;
    let window =
        |i: usize, e: usize| text_of(folded, i.saturating_sub(80), (e + 80).min(folded.len()));
    // What the rate is a rate of sits at the head of its label, further away
    // than the word that would make a share of it: « le déficit fonctionnel
    // permanent imputable à sa maladie radio-induite » before « évalué à 20 % ».
    let noise =
        |i: usize, e: usize| text_of(folded, i.saturating_sub(160), (e + 80).min(folded.len()));
    let reducing = |w: &str, n: &str| {
        REDUCTION_WORDS.iter().any(|x| w.contains(x))
            && !REDUCTION_NOISE.iter().any(|x| n.contains(x))
    };
    for i in from..folded.len() {
        if folded[i] != '%' {
            continue;
        }
        let mut j = i;
        while j > 0 && folded[j - 1] == ' ' {
            j -= 1;
        }
        if j > 0 && folded[j - 1].is_ascii_digit() && reducing(&window(i, i + 1), &noise(i, i + 1))
        {
            return true;
        }
    }
    let tail: String = folded[from..].iter().collect();
    FRACTIONS.iter().any(|f| {
        tail.match_indices(f).any(|(b, _)| {
            let s = from + tail[..b].chars().count();
            let e = s + f.chars().count();
            reducing(&window(s, e), &noise(s, e))
        })
    })
}

/// A « 0 » the label is priced at, which the amount lexer never emits.
fn zero_after(folded: &[char], from: usize, limit: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i < limit && matches!(folded[i], ' ' | ':' | '-' | '*' | '\'') {
        i += 1;
    }
    if i >= limit || folded[i] != '0' {
        return None;
    }
    let mut e = i + 1;
    if e < limit && matches!(folded[e], ',' | '.') && folded.get(e + 1) == Some(&'0') {
        e += 1;
        while e < limit && folded[e] == '0' {
            e += 1;
        }
    }
    let next = folded.get(e);
    if next.is_some_and(|c| c.is_ascii_digit() || matches!(c, ',' | '.')) {
        return None;
    }
    Some((i, e))
}

/// A chef opens on a capital, or on a list mark; a verb after « qu'il » or in
/// the past participle recites the judgment and opens nothing.
fn word_at(folded: &[char], p: usize) -> String {
    folded[p..]
        .iter()
        .take_while(|c| c.is_alphabetic())
        .collect()
}

/// « Le centre hospitalier versera à Mme A la somme de … » : the administrative
/// order condemns in the future indicative.
const FUTURE_PAYS: [&str; 6] = [
    "versera",
    "verseront",
    "paiera",
    "paieront",
    "reglera",
    "regleront",
];

/// « Article 4 : » opening a chef of the administrative dispositif. The number
/// and the colon are both required, and nothing of the sentence may run into
/// it: « l'article 700 du code de procédure civile » names a text, not a chef.
fn article_head(chars: &[char], folded: &[char], p: usize) -> bool {
    if !leftovers::word_at(folded, p, "article") {
        return false;
    }
    let prev = chars[..p].iter().rev().find(|c| **c != ' ');
    if prev.is_some_and(|c| c.is_alphanumeric() || *c == '\'') {
        return false;
    }
    let mut i = p + "article".len();
    while folded.get(i) == Some(&' ') {
        i += 1;
    }
    let digits = i;
    while folded.get(i).is_some_and(char::is_ascii_digit) {
        i += 1;
    }
    if i == digits {
        return false;
    }
    // « Article 1er : » ordinals its first one.
    while folded.get(i).is_some_and(|c| c.is_alphabetic()) {
        i += 1;
    }
    while folded.get(i) == Some(&' ') {
        i += 1;
    }
    folded.get(i) == Some(&':')
}

fn head_kind(chars: &[char], folded: &[char], p: usize, mode: Mode) -> Option<Kind> {
    let word = word_at(folded, p);
    let kind = match word.as_str() {
        "rejet" => return None,
        w if RECITAL_HEADS.contains(&w) => Kind::Recital,
        "condamne" | "condamnes" | "condamnee" | "condamnees" | "alloue" | "accorde"
        | "attribue" | "octroie" => Kind::Condemn,
        w if FUTURE_PAYS.contains(&w) => Kind::Condemn,
        "arrete" | "indemnise" if mode.extended() => Kind::Fix,
        "fixe" | "evalue" | "liquide" | "chiffre" | "rappelle" => Kind::Fix,
        "deboute" | "rejette" => Kind::Reject,
        _ => Kind::Other,
    };
    // « est condamné à verser » : the administrative order condemns in the
    // passive present and in the middle of its sentence, where the recital of a
    // judgment writes « a condamné » and a judicial chef opens its own.
    let lead: String = folded[p.saturating_sub(6)..p].iter().collect();
    // The administrative order opens its condemnation in the middle of its
    // sentence: in the passive present, where the recital of a judgment writes
    // « a condamné », or in the future indicative, which names no condemnation.
    let passive = kind == Kind::Condemn
        && ((word.starts_with("condamne") && (lead.ends_with("est ") || lead.ends_with("sont ")))
            || FUTURE_PAYS.contains(&word.as_str()));
    let last = chars[p + word.chars().count() - 1];
    if matches!(last, 'é' | 'É') && kind != Kind::Recital && !passive {
        return None;
    }
    let before = text_of(folded, p.saturating_sub(40), p);
    if RECITAL_BEFORE.iter().any(|w| before.contains(w)) {
        return None;
    }
    if passive || chars[p].is_uppercase() {
        return Some(kind);
    }
    let prev = chars[..p].iter().rev().find(|c| **c != ' ');
    matches!(
        prev,
        None | Some('-' | ';' | '.' | ':' | ',' | '*' | '•' | '>' | '|' | '\n')
    )
    .then_some(kind)
}

enum Person {
    Single(String),
    /// A group claiming the estate: the victim is the deceased it names.
    Heirs(String),
    Several,
    Organism,
}

/// The deceased a group of heirs claims for, when the payee names one.
fn deceased(raw: &str) -> Option<String> {
    for lead in SUCCESSORAL {
        let Some(i) = raw.find(lead) else { continue };
        let rest = raw[i + lead.len()..].trim_start();
        for de in ["de ", "du ", "d'"] {
            if let Some(tail) = rest.strip_prefix(de) {
                return match person(tail, Mode::Sure) {
                    Person::Single(name) => Some(name),
                    _ => Some(String::new()),
                };
            }
        }
        return Some(String::new());
    }
    None
}

/// The byte length of the civility `s` opens with, « M. [X] » as « M.[X] ».
fn civility(s: &str) -> Option<usize> {
    CIVILITIES.iter().find_map(|civ| {
        if s.starts_with(civ) {
            Some(civ.len())
        } else {
            let bare = civ.trim_end();
            (s.starts_with(bare) && s[bare.len()..].starts_with('[')).then_some(bare.len())
        }
    })
}

/// The party a payment names, folded, civility dropped: a natural person, or not.
fn person(raw: &str, mode: Mode) -> Person {
    let mut name = raw.trim();
    for stop in NAME_STOPS {
        if let Some(i) = name.find(stop) {
            name = name[..i].trim();
        }
    }
    let mut phrase = raw.trim();
    for stop in &NAME_STOPS[1..] {
        if let Some(i) = phrase.find(stop) {
            phrase = phrase[..i].trim();
        }
    }
    let phrase = phrase
        .strip_suffix(" et a")
        .or_else(|| phrase.strip_suffix(" et aux"))
        .unwrap_or(phrase);
    let joined = phrase.split(" et ").skip(1).any(|part| {
        let part = part.trim_start_matches("aux ").trim_start_matches("a ");
        part.starts_with('[')
            || civility(part).is_some()
            || JOINED.iter().any(|w| part.starts_with(w))
            || part.chars().take(40).any(|c| c == '[')
    });
    if let Some(ward) = represented(raw) {
        return person(ward, mode);
    }
    let group = REPRESENTING.iter().any(|w| raw.contains(w))
        || joined
        || phrase.contains("ensemble")
        || name.starts_with("les ")
        || name.starts_with("aux ")
        || name.starts_with("chacun")
        // « aux ayants-droits de M. [B] [G] » : the payee start eats the
        // preposition, and what remains opens on the group itself.
        || SUCCESSORAL.iter().any(|w| name.starts_with(w));
    if group {
        return match deceased(raw) {
            Some(name) if mode.extended() || !name.is_empty() => Person::Heirs(name),
            _ => Person::Several,
        };
    }
    let civil = civility(name).is_some();
    if let Some(n) = civility(name) {
        name = name[n..].trim();
    }
    if !civil && !name.starts_with('[') {
        return Person::Organism;
    }
    let bare = name.trim_matches(|c: char| !c.is_alphanumeric() && c != '[' && c != ']');
    let name = trim_name(bare);
    // « Mme D A », « M. B » : the administrative order pseudonymises by
    // initials, where the judicial one writes « [A] [W] ». The civility carries
    // them, and a name of one letter is one only behind it.
    let name = if name.is_empty() && civil {
        bare.split(|c: char| !c.is_alphabetic())
            .filter(|t| !t.is_empty())
            .take_while(|t| t.chars().count() == 1)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        name
    };
    // « M. C... », « Mme F...D... » : one clause keeps the dots where the next
    // one loses them, and two spellings of a pseudonym are two payees.
    let name = name
        .split('.')
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() || name.starts_with("[adresse") || name.starts_with("[localite") {
        Person::Organism
    } else {
        Person::Single(name)
    }
}

fn represented(raw: &str) -> Option<&str> {
    let at = raw.find("qualite")?;
    let tail = &raw[at..];
    let role = WARDENS
        .iter()
        .filter_map(|w| tail.find(w).filter(|i| *i < 40).map(|i| i + w.len()))
        .min()?;
    let rest = tail[role..].trim_start();
    let mut rest = rest
        .strip_prefix("de ")
        .or_else(|| rest.strip_prefix("d'"))?;
    while let Some(r) = KIN.iter().find_map(|k| rest.strip_prefix(k)) {
        rest = r;
    }
    Some(rest)
}

/// The natural persons a payee phrase lists, when it lists nothing else:
/// « Mme X et M. Y, chacun ».
fn each_of(raw: &str, mode: Mode) -> Option<Vec<String>> {
    let mut phrase = raw.trim();
    for stop in &NAME_STOPS[1..] {
        if let Some(i) = phrase.find(stop) {
            phrase = phrase[..i].trim();
        }
    }
    let names: Vec<String> = phrase
        .split(" et ")
        .flat_map(|p| p.split(", "))
        .map(|p| p.trim().trim_start_matches("a ").trim_start_matches("aux "))
        .filter(|p| !p.is_empty() && !p.starts_with("chacun"))
        .map(|p| match person(p, mode) {
            Person::Single(name) => Some(name),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (names.len() >= 2).then_some(names)
}

/// « à compter du 1er janvier 2018 jusqu'à la date de consolidation : » : a
/// period, and nothing else, between a label and its sum.
fn period_only(gap: &str) -> bool {
    const PERIOD: [&str; 16] = [
        "a",
        "compter",
        "du",
        "de",
        "la",
        "date",
        "au",
        "jusqu'a",
        "jusqu'au",
        "depuis",
        "le",
        "et",
        "consolidation",
        "1er",
        "pour",
        "periode",
    ];
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
    let words: Vec<&str> = gap
        .split(|c: char| c.is_whitespace() || c == ':' || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    words
        .iter()
        .any(|w| MONTHS.contains(w) || *w == "consolidation")
        && words.iter().all(|w| {
            PERIOD.contains(w)
                || MONTHS.contains(w)
                || (w.len() <= 4 && w.chars().all(|c| c.is_ascii_digit()))
        })
}

/// The tokens of a name: bracketed placeholders, a redacted or plain surname,
/// and the words that join a married name; the sentence after them is dropped.
fn trim_name(name: &str) -> String {
    const JOINS: [&str; 8] = [
        "epouse", "epse", "ep.", "ep", "veuve", "vve", "nee", "divorcee",
    ];
    let mut out: Vec<&str> = Vec::new();
    for (k, tok) in name.split_whitespace().enumerate() {
        let plain = k == 0 && tok.len() >= 2 && tok.chars().all(|c| c.is_alphabetic() || c == '-');
        if tok.starts_with('[') || tok.ends_with("...") || JOINS.contains(&tok) || plain {
            out.push(tok);
        } else {
            break;
        }
    }
    while out.last().is_some_and(|t| JOINS.contains(t)) {
        out.pop();
    }
    out.join(" ")
}

/// The first person `text` introduces by one of `leads`: byte offsets of the
/// lead and of the person.
fn mention(text: &str, leads: &[&str]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for lead in leads {
        let mut from = 0;
        while let Some(i) = text[from..].find(lead).map(|i| i + from) {
            let at = i + lead.len();
            if text[at..].starts_with('[') || civility(&text[at..]).is_some() {
                if best.is_none_or(|(b, _)| i < b) {
                    best = Some((i, at));
                }
                break;
            }
            from = at;
        }
    }
    best
}

/// The victim a fixing clause names in its opening: « de M. X », « subis par Mme Y ».
fn fixed_person(intro: &str, mode: Mode) -> Option<Person> {
    mention(intro, &["de ", "d'", "par ", "a "]).map(|(_, at)| person(&intro[at..], mode))
}

/// A natural person a row names after its label, « préjudice d'affection de
/// Mme [X] », with the char span of the mention in `text`.
fn named_in(text: &str, mode: Mode) -> Option<(String, usize, usize)> {
    let (b, at) = mention(text, &["de ", "d'", "pour ", "par "])?;
    let Person::Single(name) = person(&text[at..], mode) else {
        return None;
    };
    let e = at + text[at..].find(name.as_str())? + name.len();
    Some((name, text[..b].chars().count(), text[..e].chars().count()))
}

#[derive(Clone, Copy, PartialEq)]
enum RowMark {
    Victim,
    Tiers,
    Plain,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    Pv,
    Alloue,
    /// No third-party payer shares the poste: the two fields carry one figure.
    Both,
    /// The text does not say which field the figure fills: the poste writes none.
    Unknown,
    /// A third-party payer's share of the poste: the full amount carries it.
    Tiers,
    None,
}

#[derive(Clone)]
struct Line {
    code: &'static str,
    label: String,
    amount: f64,
    field: Field,
    /// The amount is the arrears of a rente the court serves, with its period:
    /// a component of the poste, never its capital.
    rente: Option<Option<&'static str>>,
    /// Where the clause writing it starts: the same figure on the same code
    /// in another clause repeats it.
    clause: usize,
}

/// A poste label of the motifs: code, span, and the end of its paragraph when
/// that paragraph prices something under the title of another poste.
type Label = (&'static str, usize, usize, Option<usize>);

/// What the motifs price for a person their section names, or for nobody.
struct MotifLine {
    name: Option<String>,
    ricochet: bool,
    line: Line,
}

/// Who a section of the motifs is about.
enum Section {
    Person(String),
    /// The relatives of the person it names: nobody the record holds by name.
    Relatives,
    /// Several persons or a body.
    Group,
}

#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Draft(usize),
    List(usize),
    None,
}

enum ObKind {
    Poste(String),
    Indemnite,
    Net,
    Total,
    Hors(&'static str),
}

struct Ob {
    montant: f64,
    s: usize,
    e: usize,
    kind: ObKind,
    owner: Owner,
}

#[derive(Default)]
struct Draft {
    name: String,
    /// The kinship the chef writes in apposition to the payee's name.
    lien: Option<Lien>,
    ricochet: bool,
    /// The heirs claim the deceased's own damages: the victim is the direct one.
    successoral: bool,
    lines: Vec<Line>,
    /// What the motifs price for this person, written where the dispositif is silent.
    motifs: Vec<Line>,
    nets: Vec<f64>,
    /// Codes a line of the text could merge into without the vocabulary
    /// settling it, each with the clause that leaves it open.
    unsure: Vec<(&'static str, usize)>,
}

struct FixList {
    person: Option<Person>,
    ricochet: bool,
    successoral: bool,
    lines: Vec<Line>,
    unsure: Vec<(&'static str, usize)>,
}

enum Link {
    Poste {
        code: &'static str,
        s: usize,
        e: usize,
        share: bool,
        /// The person the row names between the label and the amount.
        person: Option<String>,
    },
    Out(&'static str),
    Global {
        strong: bool,
    },
    None,
}

struct Composer<'a> {
    scan: &'a DocScan,
    mode: Mode,
    folded: Vec<char>,
    reduced: bool,
    drafts: Vec<Draft>,
    lists: Vec<FixList>,
    obs: Vec<Ob>,
    /// The dispositif pays a third-party payer: an amount paid to the victim is
    /// then her share, not the poste in full.
    tiers_payeur: bool,
    /// A third-party payer anywhere in the text: a patrimonial poste the
    /// dispositif fixes may then be the net of its recourse.
    doc_tiers: bool,
    /// A provision is named somewhere in the dispositif: a global sum paid
    /// without saying so may already deduct it.
    provisioned: bool,
    /// The dispositif prices one list before the reduction and one after: the
    /// first is the poste, the second the share, and neither is both.
    split_reduced: bool,
    /// Codes a confirmed chief of the judgment names: the appeal may add to them.
    confirmed: Vec<&'static str>,
    /// The other payees of a segment whose sum each of them takes.
    companions: Vec<usize>,
    /// Where the clause being read opens, a condemnation split out of it included.
    clause_start: usize,
    /// Where the court's own discussion opens: before it, the parties plead.
    pleaded_end: Option<usize>,
    /// The dispositif confirms part of the judgment: what the header recites of
    /// it stays awarded, and the appeal's own lines may add to the same code.
    confirms: bool,
    /// Sums the dispositif computes once the provisions are taken off: a
    /// condemnation equal to one is that balance, not a net.
    soldes: Vec<f64>,
    /// The arrears of a rente the clause serves without a poste: an indemnity,
    /// never the net.
    rentes: Vec<usize>,
    /// Obligations read as the whole a person receives because their clause
    /// pays them that alone, with the clause span: another clause paying the
    /// same person takes the reading back.
    lone_nets: Vec<(usize, usize, usize)>,
    /// Starts of the payment clauses.
    condemns: Vec<(usize, String)>,
}

impl Composer<'_> {
    /// The draft of a payee the dispositif already named, « M. [G] [K] » and
    /// « M. [K] » being one party when no other draft shares the shorter name.
    fn known(&self, name: &str) -> Option<usize> {
        if let Some(i) = self.drafts.iter().position(|d| d.name == name) {
            return Some(i);
        }
        let tokens: Vec<&str> = name.split(' ').collect();
        let nested: Vec<usize> = self
            .drafts
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                let theirs: Vec<&str> = d.name.split(' ').collect();
                !d.name.is_empty()
                    && (tokens.iter().all(|t| theirs.contains(t))
                        || theirs.iter().all(|t| tokens.contains(t)))
            })
            .map(|(i, _)| i)
            .collect();
        if let [i] = nested[..] {
            return Some(i);
        }
        // A family name several parties share names the one the dispositif pays most.
        let mut by_lines: Vec<usize> = nested.clone();
        by_lines.sort_by_key(|i| std::cmp::Reverse(self.drafts[*i].lines.len()));
        if let [first, second, ..] = by_lines[..] {
            if self.drafts[first].lines.len() > self.drafts[second].lines.len() {
                return Some(first);
            }
        }
        None
    }

    fn draft(&mut self, name: &str) -> usize {
        if let Some(i) = self.known(name) {
            return i;
        }
        self.drafts.push(Draft {
            name: name.to_string(),
            ..Default::default()
        });
        self.drafts.len() - 1
    }

    fn text(&self, s: usize, e: usize) -> String {
        text_of(&self.folded, s, e)
    }

    /// A line break between `s` and `e`: the folded text has none left.
    /// A provision the clause grants, not one it nets off: « imputation des
    /// provisions versées » qualifies nothing.
    fn provision_cue(&self, x: &Event) -> bool {
        let Event::Cue(c) = x else { return false };
        if c.cue != Cue::OutOfScope("provision") {
            return false;
        }
        let before = self.text(c.s.saturating_sub(45), c.s);
        let after = self.text(c.e, (c.e + 22).min(self.folded.len()));
        !NETTING.iter().any(|w| before.contains(w))
            && !NETTED.iter().any(|w| after.trim_start().starts_with(w))
    }

    fn breaks(&self, s: usize, e: usize) -> bool {
        e > s && self.scan.breaks()[s..e].contains(&true)
    }

    /// Only allowed words between `s` and `e`, parentheses skipped, no digit.
    fn gap_ok(&self, s: usize, e: usize, allowed: &[&str]) -> bool {
        if e < s || e - s > 80 {
            return false;
        }
        Self::words_ok(&self.folded[s..e], allowed)
    }

    fn words_ok(gap: &[char], allowed: &[&str]) -> bool {
        let mut depth = 0usize;
        let mut word = String::new();
        for &c in gap {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth > 0 => {}
                c if c.is_alphanumeric() => word.push(c),
                _ => {
                    if !word.is_empty() && !allowed.contains(&word.as_str()) {
                        return false;
                    }
                    word.clear();
                }
            }
        }
        word.is_empty() || allowed.contains(&word.as_str())
    }

    /// `Some(strong)` when the sum repairs the prejudice as a whole rather than a poste.
    fn global_after(&self, from: usize) -> Option<bool> {
        let text = self.text(from, (from + 90).min(self.folded.len()));
        let candidates = GLOBAL_STRONG
            .iter()
            .map(|p| (*p, false, true))
            .chain(GLOBAL.iter().map(|p| (*p, false, false)))
            .chain(GLOBAL_TERMINATED.iter().map(|p| (*p, true, false)));
        for (phrase, terminated, strong) in candidates {
            let Some(b) = text.find(phrase) else { continue };
            let k = text[..b].chars().count();
            if k > 40 || !self.gap_ok(from, from + k, ARTICLES) {
                continue;
            }
            let rest = &text[b + phrase.len()..];
            if !terminated
                || rest.trim().is_empty()
                || TERMINATORS.iter().any(|t| rest.starts_with(t))
            {
                return Some(strong);
            }
        }
        None
    }

    /// Codes the clause leaves unsettled: a label with no amount, or a family word
    /// (« tierce personne », « frais ») outside every label.
    fn unsure_codes(
        &self,
        ev: &[Event],
        s: usize,
        e: usize,
        linked: &[usize],
    ) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for x in ev {
            let Event::Cue(c) = x else { continue };
            match c.cue {
                Cue::Poste(code) => {
                    spans.push((c.s, c.e));
                    if !linked.contains(&c.s) {
                        out.push(code);
                    }
                }
                Cue::OutOfScope(_) => spans.push((c.s, c.e)),
                _ => {}
            }
        }
        let text = self.text(s, e);
        for (word, codes) in vocabulary::AMBIGUOUS {
            let loose = text.match_indices(word).any(|(b, _)| {
                let at = s + text[..b].chars().count();
                !spans.iter().any(|(ps, pe)| at >= *ps && at < *pe)
            });
            if loose {
                out.extend(codes.iter().copied());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Where the currency of `a` ends, its abbreviation dot included.
    fn past_currency(&self, a: &Amount) -> usize {
        let f = &self.folded;
        let mut i = a.end;
        while f.get(i) == Some(&' ') {
            i += 1;
        }
        if self.scan.chars().get(i) == Some(&'€') {
            return i + 1;
        }
        let Some(w) = leftovers::CURRENCIES
            .iter()
            .filter(|w| leftovers::word_at(f, i, w))
            .max_by_key(|w| w.len())
        else {
            return a.end;
        };
        i += w.chars().count();
        if f.get(i) == Some(&'.') && !f.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        i
    }

    /// « X au titre de P » : the amount, then the link word, then the label.
    fn forward(&self, ev: &[Event], i: usize) -> Link {
        let Event::Amount(a) = ev[i] else {
            return Link::None;
        };
        let Some(Event::Cue(u)) = ev.get(i + 1) else {
            return Link::None;
        };
        let ae = self.past_currency(&a);
        // « 6 982,99 euros (frais divers) » : the label in brackets after the sum.
        // « 680 € au titre du DFT ……… 235 € » : the leaders bind the label to
        // the figure after it, whatever stands in front.
        let bound_ahead = |pe: usize, n: usize| matches!(ev.get(n), Some(Event::Amount(b)) if leaders(&self.text(pe, b.start)));
        if let Cue::Poste(code) = u.cue {
            let gap = self.text(ae, u.s);
            let gap = gap.trim();
            if bound_ahead(u.e, i + 2) {
                return Link::None;
            }
            // « 4 000 euros de souffrances endurées » : the sum, « de », the poste.
            if de_led(gap) {
                let until = ev.get(i + 2).map_or(u.e + 40, |n| n.pos()).min(u.e + 40);
                return match qualified(code, &self.text(u.e, until.min(self.folded.len()))) {
                    Some(code) => Link::Poste {
                        code,
                        s: u.s,
                        e: u.e,
                        share: false,
                        person: None,
                    },
                    None => Link::None,
                };
            }
            let close = self.text(u.e, (u.e + 60).min(self.folded.len()));
            let tail = close.find(')').map(|k| &close[..k]);
            let next_cue = ev.get(i + 2).map_or(usize::MAX, Event::pos);
            return match tail {
                Some(tail)
                    if gap == "("
                        && !tail.chars().any(|c| c.is_ascii_digit())
                        && next_cue >= u.e + tail.chars().count() =>
                {
                    match qualified(code, tail) {
                        Some(code) => Link::Poste {
                            code,
                            s: u.s,
                            e: u.e,
                            share: false,
                            person: None,
                        },
                        None => Link::None,
                    }
                }
                _ => Link::None,
            };
        }
        if u.cue != Cue::Under
            || !self.gap_ok(ae, u.s, CONNECTORS)
            || self.breaks(ae, u.s)
            || self.text(ae, u.s).contains([';', '.'])
            || SEPARATORS
                .iter()
                .any(|sep| self.text(ae, u.s).contains(sep))
        {
            return Link::None;
        }
        match ev.get(i + 2) {
            Some(Event::Cue(p)) if self.gap_ok(u.e, p.s, ARTICLES) => match p.cue {
                Cue::Poste(_) if bound_ahead(p.e, i + 3) => return Link::None,
                Cue::Poste(code) => {
                    let until = ev.get(i + 3).map_or(p.e + 40, |n| n.pos()).min(p.e + 40);
                    let Some(code) = qualified(code, &self.text(p.e, until.min(self.folded.len())))
                    else {
                        return Link::None;
                    };
                    return Link::Poste {
                        code,
                        s: p.s,
                        e: p.e,
                        share: false,
                        person: None,
                    };
                }
                Cue::OutOfScope(cause) => return Link::Out(cause),
                _ => {}
            },
            Some(Event::Cue(p)) if p.s >= u.e && p.s - u.e <= 40 => {
                if let Cue::OutOfScope(cause @ ("tiers_payeur" | "rente_servie" | "autre_litige")) =
                    p.cue
                {
                    if prose_only(&self.text(u.e, p.s)) && !self.breaks(u.e, p.s) {
                        return Link::Out(cause);
                    }
                }
            }
            _ => {}
        }
        match self.global_after(u.e) {
            Some(strong) => Link::Global { strong },
            None => Link::None,
        }
    }

    /// « P : X » or « P à la somme de X » : the label, then the amount, nothing
    /// else between but the sum word, or the person the row names.
    fn backward(&self, ev: &[Event], i: usize) -> Link {
        let Event::Amount(a) = ev[i] else {
            return Link::None;
        };
        if i == 0 {
            return Link::None;
        }
        let Event::Cue(mut p) = ev[i - 1] else {
            return Link::None;
        };
        let mut j = i - 1;
        let mut to = a.start;
        if matches!(p.cue, Cue::Sum | Cue::Total) && j > 0 && self.gap_ok(p.e, a.start, AFTER_SUM) {
            let Event::Cue(q) = ev[j - 1] else {
                return Link::None;
            };
            if p.cue == Cue::Total && !matches!(q.cue, Cue::Poste(_)) {
                return Link::None;
            }
            // « à la somme de - 16 919,16 euros » : a negative balance, not an award.
            if self.text(p.e, a.start).trim() == "-" {
                return Link::None;
            }
            to = p.s;
            p = q;
            j -= 1;
        }
        if to < p.e || to - p.e > 80 {
            return Link::None;
        }
        let gap = self.text(p.e, to);
        if SEPARATORS.iter().any(|sep| gap.contains(sep)) {
            return Link::None;
        }
        let mut words: Vec<char> = self.folded[p.e..to].to_vec();
        let person = named_in(&gap, self.mode).map(|(name, s, e)| {
            words[s..e].iter_mut().for_each(|c| *c = ' ');
            name
        });
        if !Self::words_ok(&words, AFTER_LABEL) && !period_only(&gap) {
            // A third-party payer's figure reads through a few words of prose.
            return match p.cue {
                Cue::OutOfScope(cause @ ("tiers_payeur" | "rente_servie" | "autre_litige"))
                    if prose_only(&gap) && !self.breaks(p.e, to) =>
                {
                    Link::Out(cause)
                }
                _ => Link::None,
            };
        }
        let code = match p.cue {
            Cue::Poste(code) => code,
            Cue::OutOfScope(cause) => return Link::Out(cause),
            _ => return Link::None,
        };
        let lead_from = if j >= 1 { ev[j - 1].end() } else { 0 };
        let lead = self.text(lead_from.max(p.s.saturating_sub(40)), p.s);
        // A label folded into the one before it: that one prices the whole.
        if lead.contains("compris") || lead.contains("inclu") {
            return match (j >= 1).then(|| ev[j - 1]) {
                Some(Event::Cue(outer)) if outer.e <= p.s && p.s - outer.e <= 40 => {
                    match outer.cue {
                        Cue::Poste(code) => match qualified(code, &self.text(outer.e, p.s)) {
                            Some(code) => Link::Poste {
                                code,
                                s: outer.s,
                                e: outer.e,
                                share: false,
                                person,
                            },
                            None => Link::None,
                        },
                        _ => Link::None,
                    }
                }
                _ => Link::None,
            };
        }
        // Two labels in a row: the first is a heading, unless it closed at nothing.
        if j >= 1 && matches!(ev[j - 1].cue(), Some(Cue::Poste(_))) {
            let between = self.text(ev[j - 1].end(), p.s);
            if !EMPTY_ROW.iter().any(|w| between.contains(w)) {
                return Link::None;
            }
        }
        let unbroken = |from: usize, to: usize| {
            let lead = unparenthesised(&self.text(from, to));
            to >= from
                && lead.chars().count() <= 30
                && !SEPARATORS.iter().any(|sep| lead.contains(sep))
                && !self.breaks(from, to)
        };
        // « au titre de X 56 euros au titre de Y 396 euros » hangs nothing: a
        // figure that already carries its own label, written before it, takes
        // no second one, and the hanger opens the line of the figure after.
        // Where the label of the figure at `k` ends, when one stands in front of
        // it: a few words away, or across the leaders of a tabulated line,
        // « au titre du DFT ……… 459,20 € ».
        let own_label_end = |k: usize| {
            let Event::Amount(a) = ev[k] else {
                return None;
            };
            let mut b = k;
            while b >= 1 && matches!(ev[b - 1].cue(), Some(Cue::Under)) {
                b -= 1;
            }
            if b == 0 || !matches!(ev[b - 1].cue(), Some(Cue::Poste(_))) {
                return None;
            }
            let end = ev[b - 1].end();
            (a.start >= end && (a.start - end <= 40 || leaders(&self.text(end, a.start))))
                .then_some(end)
        };
        let own_label = |k: usize| own_label_end(k).is_some();
        let label_after = |k: usize| {
            let Event::Amount(a) = ev[k] else {
                return false;
            };
            let mut b = k;
            while b + 1 < ev.len() && matches!(ev[b + 1].cue(), Some(Cue::Sum | Cue::Under)) {
                b += 1;
            }
            match ev.get(b + 1).and_then(Event::cue) {
                Some(Cue::Poste(_)) => ev[b + 1].pos() >= a.end && ev[b + 1].pos() - a.end <= 60,
                _ => false,
            }
        };
        // The item of a list opens on its label, « - au titre du DFT ……… 499 € »,
        // or on its figure, « - 4 120 € de dommages et intérêts au titre du
        // DFP » : the bullet says which, and the leaders bind a label to the
        // figure behind them. Without either, a clause writes its labels in
        // front of its figures or behind them, and a hanger only reaches back
        // where they stand behind, counted over the clause so a single line
        // does not decide for the others.
        let front = (0..ev.len()).filter(|k| own_label(*k)).count();
        let back = (0..ev.len()).filter(|k| label_after(*k)).count();
        // The figure the label may be hung on: the one right before it, or
        // before the hanger word that precedes it.
        let k = match (j >= 1).then(|| ev[j - 1]) {
            Some(Event::Amount(_)) => Some(j - 1),
            Some(Event::Cue(u)) if u.cue == Cue::Under && j >= 2 => {
                matches!(ev[j - 2], Event::Amount(_)).then_some(j - 2)
            }
            _ => None,
        };
        let hung = match k.map(|k| (k, ev[k])) {
            Some((k, Event::Amount(prev))) => {
                let between = self.text(prev.end, p.s);
                // Where the label's item opens: on the hanger word, or on the
                // label itself.
                let label_head = ev[j - 1 - usize::from(k + 2 == j)].pos();
                let head = if k >= 1 && matches!(ev[k - 1].cue(), Some(Cue::Sum)) {
                    k - 1
                } else {
                    k
                };
                let lead_from = if head >= 1 {
                    ev[head - 1].end()
                } else {
                    ev[head].pos().saturating_sub(12)
                };
                if bullet_led(&self.text(prev.end, label_head))
                    || leaders(&self.text(p.e, to))
                    || own_label_end(k).is_some_and(|e| leaders(&self.text(e, prev.start)))
                {
                    false
                } else if bullet_led(&self.text(lead_from, ev[head].pos())) {
                    (HANGERS.iter().any(|h| between.contains(h))
                        || de_led(&self.text(self.past_currency(&prev), p.s)))
                        && unparenthesised(&between).chars().count() <= 60
                        && !self.breaks(prev.end, p.s)
                        && !SEPARATORS.iter().any(|sep| between.contains(sep))
                } else if k == j - 1 {
                    !(front > back && own_label(k))
                        && unbroken(prev.end, p.s)
                        && HANGERS.iter().any(|h| between.contains(h))
                } else {
                    // « au titre de son préjudice corporel se décomposant
                    // comme suit : » : the colon opens the list, and the
                    // hanger reaches no label past it.
                    let past_hanger = self.text(ev[j - 1].end(), p.s);
                    !(front > back && own_label(k))
                        && unbroken(prev.end, ev[j - 1].pos())
                        && !past_hanger.contains([':', ';'])
                        && !self.breaks(ev[j - 1].end(), p.s)
                }
            }
            _ => false,
        };
        if hung {
            return Link::None;
        }
        let Some(code) = qualified(code, &gap) else {
            return Link::None;
        };
        Link::Poste {
            code,
            s: p.s,
            e: p.e,
            share: SHARE_WORDS.iter().any(|w| gap.contains(w)),
            person,
        }
    }

    fn link(&self, ev: &[Event], i: usize, fixing: bool) -> Link {
        let Event::Amount(a) = ev[i] else {
            return Link::None;
        };
        let forward = self.forward(ev, i);
        let backward = self.backward(ev, i);
        match (&forward, &backward) {
            // « au titre du DFT ……… 235 € au titre des SE » : the leaders bind
            // the figure to the label in front of it.
            (Link::Poste { .. }, Link::Poste { e, .. }) if leaders(&self.text(*e, a.start)) => {
                return backward
            }
            (Link::Poste { .. }, Link::Poste { .. }) => return Link::None,
            // « la somme provisionnelle de X au titre du PAF » stays a provision.
            (Link::Poste { .. }, Link::Out("provision")) => return backward,
            (Link::None, Link::Poste { .. } | Link::Out(_)) => return backward,
            (Link::None, _) => {}
            _ => return forward,
        }
        if fixing {
            if let Some(Event::Cue(p)) = ev.get(i + 1) {
                if let Cue::Poste(code) = p.cue {
                    let ae = self.past_currency(&a);
                    let ae = ae + usize::from(self.folded.get(ae) == Some(&','));
                    let gap = self.text(ae, p.s);
                    if self.gap_ok(ae, p.s, BEFORE_LABEL)
                        && !gap.contains([';', ',', '.', ':'])
                        && gap
                            .split(|c: char| !c.is_alphanumeric())
                            .any(|w| ARTICLES.contains(&w) && w != "euros")
                    {
                        return Link::Poste {
                            code,
                            s: p.s,
                            e: p.e,
                            share: false,
                            person: None,
                        };
                    }
                }
            }
        }
        Link::None
    }

    /// The lead of the amount says the clause takes it off for a third-party
    /// payer: what a caisse or a mutuelle received or is owed.
    fn deducted_of_tiers(&self, ev: &[Event], i: usize, a: &Amount) -> bool {
        let from = if i > 0 { ev[i - 1].end() } else { 0 };
        let lead = self.text(from.max(a.start.saturating_sub(100)), a.start);
        if SEPARATORS.iter().any(|sep| lead.contains(sep)) {
            return false;
        }
        [
            "deduction",
            "percues",
            "percue",
            "versees par",
            "versee par",
        ]
        .iter()
        .any(|w| lead.contains(w))
            && !lead.contains("provision")
            && DEDUCTED_OF.iter().any(|w| lead.contains(w))
    }

    /// A label of its own before the amount, one the vocabulary does not hold:
    /// the figure is not the row's.
    fn foreign_label(&self, ev: &[Event], i: usize, a: &Amount) -> bool {
        let from = if i > 0 { ev[i - 1].end() } else { 0 };
        let mut lead = self.text(from.max(a.start.saturating_sub(60)), a.start);
        if let Some(cut) = SEPARATORS
            .iter()
            .filter_map(|w| lead.rfind(w).map(|b| b + w.len()))
            .max()
        {
            lead = lead[cut..].to_string();
        }
        let opened = [" -", " *", " •", " >"].iter().any(|m| lead.contains(m))
            || self.breaks(from.max(a.start.saturating_sub(60)), a.start);
        let Some(colon) = lead.rfind(':') else {
            return opened;
        };
        opened
            || lead[..colon]
                .split(|c: char| !c.is_alphabetic())
                .filter(|w| w.len() >= 3)
                .count()
                >= 2
    }

    /// « frais divers (2 010 euros … ) » : an amount inside the parenthesis that
    /// follows the label is a component the poste already prices.
    fn component(&self, label_end: usize, a: &Amount) -> bool {
        label_end <= a.start && self.text(label_end, a.start).contains('(')
    }

    /// Who the row names right after the amount: « 21,45 euros en faveur de la
    /// victime ; 11 103,81 euros en faveur de la CPAM ».
    fn row_mark(&self, ev: &[Event], i: usize, a: &Amount) -> RowMark {
        let cap = (a.end + 60).min(self.folded.len());
        let until = ev.get(i + 1).map_or(cap, |n| n.pos().min(cap));
        let mut after = self.text(a.end, until);
        if let Some(cut) = SEPARATORS.iter().filter_map(|sep| after.find(sep)).min() {
            after.truncate(cut);
        }
        if let Some(cut) = after.find("compris") {
            after.truncate(cut);
        }
        // The lead is what hangs on this amount: after a separator the words are
        // its own, straight after another amount they qualify that one.
        let from = if i > 0 { ev[i - 1].end() } else { 0 };
        let mut lead = self.text(from.max(a.start.saturating_sub(50)), a.start);
        let cut = |words: &[&str]| {
            words
                .iter()
                .filter_map(|w| lead.rfind(w).map(|b| b + w.len()))
                .max()
        };
        let sep = cut(&SEPARATORS);
        let currency = cut(&["euros", "euro", "€", "cfp"]);
        let after_amount = i > 0 && matches!(ev[i - 1], Event::Amount(_));
        match (sep, currency) {
            (None, _) if after_amount => lead.clear(),
            (None, None) => {}
            (sep, currency) => lead = lead[sep.max(currency).unwrap_or(0)..].to_string(),
        }
        let marked = |words: &[&str]| words.iter().any(|w| after.contains(w) || lead.contains(w));
        if marked(&VICTIM_SHARE_WORDS) {
            RowMark::Victim
        } else if marked(&TIERS_PAYEUR_WORDS) {
            RowMark::Tiers
        } else {
            RowMark::Plain
        }
    }

    fn ob(&mut self, a: &Amount, kind: ObKind, owner: Owner) {
        self.obs.push(Ob {
            montant: euros(a),
            s: a.start,
            e: a.end,
            kind,
            owner,
        });
    }

    /// The cause the clause writes closest to the amount, `fallback` when it
    /// names none.
    fn nearest_cause(ev: &[Event], a: &Amount, fallback: &'static str) -> &'static str {
        ev.iter()
            .filter_map(|e| match e.cue() {
                Some(Cue::OutOfScope(c)) => Some((e.pos().abs_diff(a.start), c)),
                _ => None,
            })
            .min_by_key(|(d, _)| *d)
            .map_or(fallback, |(_, c)| c)
    }

    fn all_hors(&mut self, ev: &[Event], cause: &'static str) {
        for e in ev {
            if let Event::Amount(a) = e {
                self.ob(a, ObKind::Hors(cause), Owner::None);
            }
        }
    }

    /// Every amount of `ev` out of the record, each with the cause `scope`
    /// writes closest to it.
    fn hors_each(&mut self, scope: &[Event], ev: &[Event], fallback: &'static str) {
        for e in ev {
            if let Event::Amount(a) = e {
                let cause = Self::nearest_cause(scope, a, fallback);
                self.ob(a, ObKind::Hors(cause), Owner::None);
            }
        }
    }

    /// « intérêts au double du taux légal sur la somme de X » : X is the base of
    /// the interest, not an indemnity.
    fn interest_base(&self, a: &Amount) -> bool {
        let before = self.text(a.start.saturating_sub(80), a.start);
        before.contains("interet")
            && [
                "sur la somme",
                "sur les sommes",
                "sur le montant",
                "sur celle",
            ]
            .iter()
            .any(|w| before.contains(w))
    }

    /// The amount is what stays due once the provisions the clause names are
    /// taken off: a balance the record does not hold.
    fn solde_before(&self, ev: &[Event], i: usize, a: &Amount) -> bool {
        let provision_at = |from: usize, to: usize| {
            ev.iter().any(|x| {
                x.cue() == Some(Cue::OutOfScope("provision")) && x.pos() >= from && x.pos() < to
            })
        };
        let prev_end = ev[..i]
            .iter()
            .rev()
            .find_map(|x| match x {
                Event::Amount(p) => Some(p.end),
                _ => None,
            })
            .unwrap_or(0);
        let before = self.text(a.start.saturating_sub(60).max(prev_end), a.start);
        SOLDE_WORDS.iter().any(|w| before.contains(w))
            && provision_at(a.start.saturating_sub(160), a.start)
    }

    fn solde(&self, ev: &[Event], i: usize, a: &Amount, end: usize) -> bool {
        if self.solde_before(ev, i, a) {
            return true;
        }
        let later_balance = (i + 1..ev.len()).any(|m| match ev[m] {
            Event::Amount(b) => self.solde_before(ev, m, &b),
            Event::Cue(_) => false,
        });
        if later_balance {
            return false;
        }
        let next = ev[i + 1..]
            .iter()
            .find_map(|x| match x {
                Event::Amount(n) => Some(n.start),
                _ => None,
            })
            .unwrap_or(end)
            .min(end);
        let cap = (a.end + 140).min(next);
        let after = self.text(a.end, cap);
        let Some(made) = ["deduction faite", "apres deduction", "sous deduction"]
            .iter()
            .filter_map(|w| after.find(w))
            .min()
        else {
            return false;
        };
        // A payment still to come deducts from a gross figure.
        const TO_COME: [&str; 6] = ["seront", "sera", "devra", "conviendra", "convient", "dont"];
        if TO_COME.iter().any(|w| after[..made].contains(w)) {
            return false;
        }
        ev.iter().any(|x| {
            x.cue() == Some(Cue::OutOfScope("provision")) && x.pos() >= a.end && x.pos() < cap
        })
    }

    /// « une rente annuelle de X » : the arrears of a rente, with its period,
    /// unless the figure is the capital of that rente.
    fn rente(&self, a: &Amount, from: usize) -> Option<Option<&'static str>> {
        let before = self.text(from.max(a.start.saturating_sub(150)).min(a.start), a.start);
        // « une rentre annuelle » : the registry's typo keeps the rente.
        let at = before.rfind("rente").or_else(|| before.rfind("rentre"))?;
        let tail = &before[at..];
        if tail.contains("capital") || tail.contains("arrerages") {
            return None;
        }
        let around = format!(
            "{tail} {}",
            self.text(a.end, (a.end + 40).min(self.folded.len()))
        );
        Some(
            RENTE_PERIODS
                .iter()
                .find(|(w, _)| around.contains(w))
                .map(|(_, p)| *p),
        )
    }

    /// The last civility the text between `s` and `e` opens a person with.
    fn last_named(&self, s: usize, e: usize) -> Option<String> {
        let text = self.text(s, e);
        let at = text
            .char_indices()
            .filter(|(i, _)| *i == 0 || text.as_bytes()[i - 1] == b' ')
            .filter(|(i, _)| civility(&text[*i..]).is_some())
            .map(|(i, _)| i)
            .next_back()?;
        let named = &text[at..];
        Some(named[..named.find(" et ").unwrap_or(named.len())].to_string())
    }

    /// Where a payee opens inside a payment clause: a civility, a bracketed name,
    /// a group or a capitalised body after « à », itself after a separator or « et ».
    fn payee_starts(&self, s: usize, e: usize, listed: bool) -> Vec<usize> {
        let chars = self.scan.chars();
        let mut out = Vec::new();
        for i in s..e.saturating_sub(3) {
            let word = if self.folded[i] == 'a' && self.folded[i + 1] == ' ' {
                2
            } else if self.text(i, i + 4) == "aux " {
                4
            } else if listed
                && matches!(self.folded[i], '.' | '-' | '*' | '•')
                && self.folded[i + 1] == ' '
            {
                2
            } else {
                continue;
            };
            if i > 0 && self.folded[i - 1].is_alphanumeric() {
                continue;
            }
            let at = i + word;
            let rest = self.text(at, (at + 30).min(e));
            let opens = civility(&rest).is_some()
                || rest.starts_with('[')
                || PAYEE_GROUPS.iter().any(|g| rest.starts_with(g))
                || PAYEE_LEADS.iter().any(|l| {
                    rest.starts_with(l)
                        && (chars
                            .get(at + l.chars().count())
                            .is_some_and(|c| c.is_uppercase() || *c == '[')
                            || ORGANISM_WORDS
                                .iter()
                                .any(|w| rest[l.len()..].starts_with(w)))
                });
            if !opens {
                continue;
            }
            let before = self.text(i.saturating_sub(6).max(s), i);
            let before = before.trim_end();
            if before.is_empty()
                || before.ends_with([',', ';', ':', '-', '*', '\'', '.', ')'])
                || before.ends_with(" et")
                || before == "et"
                // « verser une somme de 321 178 euros à Mme A » : a person named
                // right behind the money is who it goes to.
                || before.ends_with("euros")
                || self.breaks(i.saturating_sub(2).max(s), i)
            {
                out.push(at);
            }
        }
        out
    }

    /// « Confirme le jugement en ce qu'il a : condamné…, fixé… » : the chiefs
    /// it keeps are awards; those past an exception, and a blanket
    /// confirmation, only recite the judgment.
    /// The chiefs of the judgment a recital keeps: « Confirme … » up to its
    /// exception, « Infirme … sauf en ce qu'il a … » from it.
    fn confirmation(&mut self, ev: &[Event], s: usize, e: usize, chars: &[char]) {
        let text = self.text(s, e);
        let confirms = text.starts_with("confirme");
        let itemised = text.contains("en ce qu");
        self.confirms |= confirms && (!itemised || text.contains("surplus"));
        let at = |b: usize| s + text[..b].chars().count();
        let (from, cut) = if confirms {
            let cut = EXCEPTIONS.iter().filter_map(|w| text.find(w)).min();
            (s, cut.map_or(e, at))
        } else {
            (text.find("sauf en ce qu").map_or(s, at), e)
        };
        let mut starts: Vec<(usize, Kind)> = Vec::new();
        for x in ev {
            let Some(Cue::Condemn | Cue::Fix | Cue::Reject | Cue::PayTo) = x.cue() else {
                continue;
            };
            let p = x.pos();
            if !itemised || p <= from || p >= cut {
                continue;
            }
            let word: String = self.folded[p..]
                .iter()
                .take_while(|c| c.is_alphabetic())
                .collect();
            if chars[p + word.chars().count() - 1] != 'é' {
                continue;
            }
            let kind = match word.as_str() {
                "condamne" => Kind::Condemn,
                w if ALLOCATES.contains(&w) => Kind::Condemn,
                "fixe" | "evalue" | "liquide" | "arrete" | "chiffre" => Kind::Fix,
                "deboute" | "rejete" => Kind::Reject,
                _ => continue,
            };
            starts.push((p, kind));
        }
        let first = starts.first().map_or(cut, |x| x.0);
        let head: Vec<Event> = ev.iter().filter(|x| x.pos() < first).copied().collect();
        self.all_hors(&head, "rappel_du_jugement");
        for (k, &(b, kind)) in starts.iter().enumerate() {
            let item_e = starts.get(k + 1).map_or(cut, |x| x.0);
            let item: Vec<Event> = ev
                .iter()
                .filter(|x| x.pos() >= b && x.pos() < item_e)
                .copied()
                .collect();
            match kind {
                Kind::Condemn => self.payment(&item, b, item_e),
                Kind::Fix => self.fixing(&item, b, item_e),
                _ => self.hors_each(&item, &item, "pretention"),
            }
        }
        let rest: Vec<Event> = ev.iter().filter(|x| x.pos() >= cut).copied().collect();
        self.all_hors(&rest, "rappel_du_jugement");
    }

    fn payment(&mut self, ev: &[Event], s: usize, e: usize) {
        let pays: Vec<usize> = ev
            .iter()
            .filter(|x| x.cue() == Some(Cue::PayTo))
            .map(Event::end)
            .collect();
        let first_pay = pays.first().copied();
        // « à payer à : . Mme X la somme de…, . Mme Y la somme de… » lists its payees.
        let listed =
            first_pay.is_some_and(|p| self.text(p, (p + 3).min(e)).trim_start().starts_with(':'));
        let mut starts: Vec<(usize, bool)> = pays.iter().map(|p| (*p, true)).collect();
        let mut named = 0usize;
        for p in self.payee_starts(s, e, listed) {
            if first_pay.is_none_or(|f| p > f) && !pays.contains(&p) {
                starts.push((p, false));
                named += 1;
            }
        }
        if listed && named > 0 {
            starts.retain(|(_, sure)| !sure);
        }
        starts.sort_unstable();
        // A payee named before any money shares what follows: « à M. X et à
        // Mme Y la somme de », unless each takes it.
        let clause_text = self.text(s, e);
        let each = clause_text.contains("chacun");
        let mut kept: Vec<usize> = Vec::new();
        for (p, sure) in starts {
            let paid = kept.last().is_some_and(|k| {
                ev.iter()
                    .any(|x| matches!(x, Event::Amount(_)) && x.pos() >= *k && x.pos() < p)
            });
            if kept.is_empty() || sure || paid || each {
                kept.push(p);
            }
        }
        let Some(&first) = kept.first() else {
            // « condamne X à payer les montants suivants : DFT : … » prices
            // the postes of whoever the dispositif is about, or of the person
            // its opening names.
            let first_label = ev
                .iter()
                .find(|x| matches!(x.cue(), Some(Cue::Poste(_))))
                .map_or(e, Event::pos);
            let intro = self.text(s, first_label.min(s + 600));
            let named =
                intro.contains('[') || CIVILITIES.iter().any(|c| intro.contains(&format!(" {c}")));
            if first_label < e && (!named || fixed_person(&intro, self.mode).is_some()) {
                return self.fixing(ev, s, e);
            }
            self.condemns.push((s, self.text(s, e)));
            return self.hors_each(ev, ev, "autre");
        };
        // The administrative order names its payee after the money, where the
        // judicial one names it before. One payee, no verb that already carries
        // it, and the sums of the clause are hers.
        let reversed = first_pay.is_none()
            && ev
                .iter()
                .any(|x| matches!(x, Event::Amount(_)) && x.pos() < first);
        let early: Vec<Event> = if reversed {
            Vec::new()
        } else {
            ev.iter().filter(|x| x.pos() < first).copied().collect()
        };
        self.hors_each(ev, &early, "autre");
        let head = self.text(s, first);
        self.condemns.push((s, head.clone()));
        let has_provision = ev.iter().any(|x| self.provision_cue(x))
            || (!self.mode.extended()
                && self.provisioned
                && !clause_text.contains("non deduite")
                && !clause_text.contains("hors deduction"));
        // « à M. X et à Mme Y la somme de 3 000 euros chacun » : the payees
        // named before the money each take what it writes.
        let mut waiting: Vec<usize> = Vec::new();
        for (k, &b) in kept.iter().enumerate() {
            let next = kept.get(k + 1).copied().unwrap_or(e);
            // Reversed, each payee takes what is written between the payee
            // before her and her own name.
            let (from_pos, to_pos) = match reversed {
                true if k == 0 => (s, b),
                true => (kept[k - 1], b),
                false => (b, next),
            };
            let seg: Vec<Event> = ev
                .iter()
                .filter(|x| x.pos() >= from_pos && x.pos() < to_pos)
                .copied()
                .collect();
            let paid = seg.iter().any(|x| matches!(x, Event::Amount(_)));
            let before: Vec<usize> = self.drafts.iter().map(|d| d.lines.len()).collect();
            let owner = self.pay_segment(ev, &seg, &head, (s, e), (b, next), has_provision);
            waiting.append(&mut self.companions);
            let Owner::Draft(d) = owner else {
                waiting.clear();
                continue;
            };
            if !paid {
                waiting.push(d);
                continue;
            }
            // « chacun » before the money covers every sum, after one that sum.
            let first_amount = seg.iter().find_map(|x| match x {
                Event::Amount(a) => Some(a.start),
                _ => None,
            });
            let lead_each = first_amount.is_some_and(|f| self.text(b, f).contains("chacun"));
            let each: Vec<f64> = seg
                .iter()
                .enumerate()
                .filter_map(|(k, x)| {
                    let Event::Amount(a) = x else { return None };
                    let until = seg[k + 1..]
                        .iter()
                        .find(|y| matches!(y, Event::Amount(_)))
                        .map_or(next, Event::pos)
                        .min(a.end + 80);
                    (lead_each || self.text(a.end, until).contains("chacun")).then_some(euros(a))
                })
                .collect();
            let from = before.get(d).copied().unwrap_or(0);
            let fresh: Vec<Line> = self.drafts[d].lines[from..].to_vec();
            for w in waiting.iter().copied().filter(|w| *w != d) {
                self.drafts[w]
                    .lines
                    .extend(fresh.iter().filter(|l| each.contains(&l.amount)).cloned());
            }
            // What the payees named before share is nobody's alone.
            if waiting.iter().any(|w| *w != d) {
                self.drafts[d].lines.truncate(from);
                self.drafts[d]
                    .lines
                    .extend(fresh.iter().filter(|l| each.contains(&l.amount)).cloned());
            }
            waiting.clear();
        }
    }

    /// One payee of the clause and the amounts written up to the next one.
    fn pay_segment(
        &mut self,
        clause: &[Event],
        ev: &[Event],
        head: &str,
        span: (usize, usize),
        (pay, end): (usize, usize),
        has_provision: bool,
    ) -> Owner {
        // A clause about costs alone: what it pays without a label is costs.
        let costs = clause
            .iter()
            .any(|x| matches!(x.cue(), Some(Cue::OutOfScope("article_700" | "depens"))))
            && !clause
                .iter()
                .any(|x| matches!(x.cue(), Some(Cue::Poste(_))));
        let stop = ev
            .iter()
            .find(|x| {
                x.pos() >= pay
                    && (matches!(x, Event::Amount(_))
                        || matches!(x.cue(), Some(Cue::Sum | Cue::Total)))
            })
            .map_or(end, Event::pos);
        let raw = self.text(pay, stop);
        // « à payer à ce dernier » : the person the clause named last.
        let raw = if ANAPHORA.iter().any(|w| raw.trim_start().starts_with(w)) {
            self.last_named(self.clause_start, pay).unwrap_or(raw)
        } else {
            raw
        };
        let lien = kinship_of(&raw);
        let owner = match person(&raw, self.mode) {
            Person::Single(name) => {
                let d = self.draft(&name);
                self.drafts[d].lien = self.drafts[d].lien.or(lien);
                Owner::Draft(d)
            }
            Person::Heirs(name) => {
                let d = self.draft(&name);
                self.drafts[d].successoral = true;
                Owner::Draft(d)
            }
            Person::Several => match each_of(&raw, self.mode) {
                Some(names) if self.text(pay, end).contains("chacun") => {
                    let d = self.draft(&names[0]);
                    for n in &names[1..] {
                        let c = self.draft(n);
                        self.companions.push(c);
                    }
                    Owner::Draft(d)
                }
                _ => Owner::None,
            },
            // What a caisse receives is its own: the label a sum carries names
            // its cause, nothing further off in the clause does.
            Person::Organism if ORGANISM_WORDS.iter().any(|w| raw.contains(w)) => {
                let amounts: Vec<Amount> = ev
                    .iter()
                    .filter_map(|x| match x {
                        Event::Amount(a) => Some(*a),
                        _ => None,
                    })
                    .collect();
                for (k, a) in amounts.iter().enumerate() {
                    let until = amounts.get(k + 1).map_or(usize::MAX, |b| b.start);
                    let cause = ev
                        .iter()
                        .find_map(|x| match x.cue() {
                            Some(Cue::OutOfScope(c))
                                if x.pos() > a.end && x.pos() < until && x.pos() - a.end <= 60 =>
                            {
                                Some(c)
                            }
                            _ => None,
                        })
                        .unwrap_or("tiers_payeur");
                    self.ob(a, ObKind::Hors(cause), Owner::None);
                }
                return Owner::None;
            }
            Person::Organism => {
                self.hors_each(clause, ev, "tiers_payeur");
                return Owner::None;
            }
        };
        let text = self.text(pay, end);
        if let Owner::Draft(d) = owner {
            self.drafts[d].ricochet |= RICOCHET_WORDS
                .iter()
                .any(|w| text.contains(w) || head.contains(w));
            self.drafts[d].successoral |= text.contains("successoral");
        }
        let total = ev
            .iter()
            .position(|x| matches!(x.cue(), Some(Cue::Sum | Cue::Total)))
            .and_then(|si| {
                let ti = si + 1;
                match ev.get(ti) {
                    Some(Event::Amount(t)) if self.gap_ok(ev[si].end(), t.start, AFTER_SUM) => {
                        Some(ti)
                    }
                    _ => None,
                }
            });
        // A total row of columns totals several things: none is the net. The
        // rente an article serves beside the capital opens no column.
        let total = total.filter(|ti| match ev.get(ti + 1) {
            Some(Event::Amount(a)) => self.rente(a, pay).is_some(),
            _ => true,
        });
        let total_after_total_cue =
            total.is_some_and(|ti| ti > 0 && ev[ti - 1].cue() == Some(Cue::Total));
        let mut linked_after_including = false;
        let mut including_seen = false;
        let mut linked: Vec<usize> = Vec::new();
        let mut row: Option<&'static str> = None;
        let mut amounts = 0usize;
        // « la somme de 21 612 euros, dont il conviendra de déduire la
        // provision de 6 000 euros » : the provision a clause takes off is no
        // second sum, and what it names is the net before provisions.
        let mut taken_off = 0usize;
        let mut past_provision = false;
        let since = |i: usize| {
            ev[..i]
                .iter()
                .rev()
                .find_map(|x| match x {
                    Event::Amount(p) => Some(p.end),
                    _ => None,
                })
                .map_or(pay, |p| p.max(pay))
        };
        for i in 0..ev.len() {
            let Event::Amount(a) = ev[i] else {
                match ev[i].cue() {
                    Some(Cue::Including) => including_seen = true,
                    Some(Cue::Poste(code)) => row = Some(code),
                    Some(Cue::OutOfScope("provision")) => {
                        past_provision = total.is_some_and(|ti| ev[ti].pos() < ev[i].pos());
                    }
                    _ => {}
                }
                continue;
            };
            let counts = usize::from(self.rente(&a, pay).is_none());
            amounts += counts;
            if past_provision {
                taken_off += counts;
            }
            let lk = self.link(ev, i, false);
            match lk {
                Link::Poste {
                    code, s, e, person, ..
                } => {
                    if including_seen {
                        linked_after_including = true;
                    }
                    linked.push(s);
                    row = Some(code);
                    let columns = matches!(ev.get(i + 1), Some(Event::Amount(_)));
                    let label = self.scan.verbatim(s, e);
                    let field = match (columns, self.mode.extended()) {
                        (true, false) => Field::Unknown,
                        (_, true) if self.component(e, &a) => Field::None,
                        (_, true) if self.tiers_payeur || self.reduced => Field::Pv,
                        (_, true) => Field::Both,
                        (false, false)
                            if !self.reduced
                                && (vocabulary::extra_patrimonial(code) || !self.doc_tiers) =>
                        {
                            Field::Both
                        }
                        (false, false) => Field::Pv,
                    };
                    let target = match person {
                        Some(name) if self.mode.extended() => Owner::Draft(self.draft(&name)),
                        _ => owner,
                    };
                    let rente = self.rente(&a, since(i));
                    if let Owner::Draft(d) = target {
                        self.drafts[d].lines.push(Line {
                            code,
                            label,
                            amount: euros(&a),
                            field,
                            rente,
                            clause: span.0,
                        });
                    }
                    self.ob(&a, ObKind::Poste(code.to_string()), target);
                }
                Link::Out(cause) => self.ob(&a, ObKind::Hors(cause), Owner::None),
                _ if self.interest_base(&a) => {
                    self.ob(&a, ObKind::Hors("intermediaire"), Owner::None)
                }
                _ if self.solde(ev, i, &a, span.1) => {
                    self.soldes.push(euros(&a));
                    self.ob(&a, ObKind::Hors("intermediaire"), Owner::None)
                }
                Link::None if costs => {
                    let cause = Self::nearest_cause(clause, &a, "article_700");
                    self.ob(&a, ObKind::Hors(cause), Owner::None)
                }
                Link::Global { strong: true } if Some(i) == total && !has_provision => {
                    self.ob(&a, ObKind::Net, owner)
                }
                _ if self.deducted_of_tiers(ev, i, &a) => {
                    self.ob(&a, ObKind::Hors("tiers_payeur"), Owner::None)
                }
                Link::Global { .. } | Link::None => match (row, self.row_mark(ev, i, &a)) {
                    (Some(code), _) if Some(i) != total && self.rente(&a, since(i)).is_some() => {
                        let rente = self.rente(&a, since(i));
                        if let Owner::Draft(d) = owner {
                            self.drafts[d].lines.push(Line {
                                code,
                                label: self.scan.verbatim(a.start, a.end),
                                amount: euros(&a),
                                field: Field::None,
                                rente,
                                clause: span.0,
                            });
                        }
                        self.ob(&a, ObKind::Poste(code.to_string()), owner);
                    }
                    (Some(code), RowMark::Victim) if Some(i) != total => {
                        if let Owner::Draft(d) = owner {
                            self.drafts[d].lines.push(Line {
                                code,
                                label: self.scan.verbatim(a.start, a.end),
                                amount: euros(&a),
                                field: Field::Pv,
                                rente: None,
                                clause: span.0,
                            });
                        }
                        self.ob(&a, ObKind::Poste(code.to_string()), owner);
                    }
                    (Some(code), RowMark::Tiers) if Some(i) != total => {
                        if let (Owner::Draft(d), true) = (owner, self.mode.extended()) {
                            self.drafts[d].lines.push(Line {
                                code,
                                label: self.scan.verbatim(a.start, a.end),
                                amount: euros(&a),
                                field: Field::Tiers,
                                rente: None,
                                clause: span.0,
                            });
                        }
                        self.ob(&a, ObKind::Poste(code.to_string()), owner);
                    }
                    _ => {
                        if self.rente(&a, span.0).is_some() {
                            self.rentes.push(a.start);
                        }
                        self.ob(&a, ObKind::Indemnite, owner)
                    }
                },
            }
        }
        if let (Some(ti), Owner::Draft(d)) = (total, owner) {
            let Event::Amount(t) = ev[ti] else {
                return owner;
            };
            // The postes listed under the sum add up to it: nothing was
            // deducted from it, whatever provision the dispositif names.
            let listed: Vec<f64> = self
                .obs
                .iter()
                .filter(|o| {
                    o.s > t.end
                        && o.e <= span.1
                        && o.owner == owner
                        && matches!(o.kind, ObKind::Poste(_))
                })
                .map(|o| o.montant)
                .collect();
            let closes =
                listed.len() >= 2 && (listed.iter().sum::<f64>() - euros(&t)).abs() < 0.005;
            // « la somme de 1 602 516,22 € … se décomposant comme suit : …
            // déduction faite des provisions de 140 000 € » : the postes add
            // up to more than the sum, which is what stays after the provision.
            let netted = linked_after_including
                && !listed.is_empty()
                && !closes
                && ev
                    .iter()
                    .any(|x| x.cue() == Some(Cue::OutOfScope("provision")));
            let ob = self.obs.iter_mut().rev().find(|o| o.s == t.start);
            if let Some(o) = ob {
                if matches!(o.kind, ObKind::Indemnite)
                    && (linked_after_including || total_after_total_cue)
                    && (!has_provision || closes)
                    && !netted
                    && !self.rentes.contains(&t.start)
                {
                    o.kind = ObKind::Net;
                }
                if matches!(o.kind, ObKind::Net) {
                    self.drafts[d].nets.push(euros(&t));
                }
            }
        }
        // One sum to one person, nothing else said of it: what she receives.
        // The rente an article serves beside the capital is not a second sum,
        // the record holding it on its own field.
        let lone = amounts.saturating_sub(taken_off) == 1;
        if let (true, true, Owner::Draft(d), Some(ti)) = (!self.reduced, lone, owner, total) {
            let Event::Amount(t) = ev[ti] else {
                return owner;
            };
            let clause_text = self.text(span.0, span.1);
            let out_cue = clause
                .iter()
                .any(|x| matches!(x.cue(), Some(Cue::OutOfScope(_))));
            // A provision the article subtracts from the sum it names leaves
            // that sum the net before provisions, which is what the record
            // holds; the solde is what remains after it, and holds nothing.
            let deducted = !self.solde_before(ev, ti, &t)
                && clause.iter().all(|x| match x.cue() {
                    Some(Cue::OutOfScope(w)) => w == "provision" && x.pos() > t.start,
                    _ => true,
                });
            if (!out_cue || clause_text.contains("non deduite") || deducted)
                && self.rente(&t, span.0).is_none()
            {
                let at = self
                    .obs
                    .iter()
                    .rposition(|o| o.s == t.start && o.owner == owner);
                if let Some(k) = at {
                    if matches!(self.obs[k].kind, ObKind::Indemnite) {
                        self.obs[k].kind = ObKind::Net;
                        self.drafts[d].nets.push(euros(&t));
                        self.lone_nets.push((k, span.0, span.1));
                    }
                }
            }
        }
        if let Owner::Draft(d) = owner {
            let unsure = self.unsure_codes(ev, pay, end, &linked);
            self.drafts[d]
                .unsure
                .extend(unsure.into_iter().map(|c| (c, span.0)));
        }
        owner
    }

    /// Where the motifs open a section about one person, « 3. Sur les préjudices
    /// de Mme [O] »: the section start and who it is about. A section about the
    /// relatives of the person it names is about nobody the record can hold.
    fn sections(&self, zs: usize, ze: usize) -> Vec<(usize, Section)> {
        const ANCHORS: [&str; 8] = [
            "sur le ",
            "sur la ",
            "sur les ",
            "sur l'",
            "s'agissant de ",
            "s'agissant du ",
            "concernant ",
            "en ce qui concerne ",
        ];
        const ABOUT: [&str; 7] = [
            "prejudice",
            "indemnisation",
            "demande",
            "liquidation",
            "reparation",
            "evaluation",
            "droits",
        ];
        let chars = self.scan.chars();
        let text = self.text(zs, ze);
        let mut out: Vec<(usize, Section)> = Vec::new();
        for anchor in ANCHORS {
            for (b, _) in text.match_indices(anchor) {
                let at = zs + text[..b].chars().count();
                let prev = chars[zs..at].iter().rev().find(|c| **c != ' ');
                let heading = chars[at].is_uppercase()
                    || matches!(prev, None | Some('\n' | '.' | ')' | '-' | '*' | ':' | '|'))
                    || prev.is_some_and(|c| c.is_ascii_digit());
                if !heading {
                    continue;
                }
                let rest = &text[b..];
                let window: String = rest.chars().take(110).collect();
                let Some((_, at_person)) = mention(&window, &["de ", "d'", "par ", "a "]) else {
                    continue;
                };
                let lead = &window[..at_person];
                if !ABOUT.iter().any(|w| lead.contains(w)) {
                    continue;
                }
                let named: String = rest[at_person..].chars().take(100).collect();
                let section = if RELATIVES_OF.iter().any(|w| lead.contains(w)) {
                    Section::Relatives
                } else {
                    match person(&named, self.mode) {
                        Person::Single(name) => Section::Person(name),
                        _ => Section::Group,
                    }
                };
                out.push((at, section));
            }
        }
        out.sort_by_key(|(at, _)| *at);
        out.dedup_by_key(|(at, _)| *at);
        out
    }

    /// What the motifs price: a paragraph naming exactly one poste of the
    /// vocabulary, and an amount the court's own award verb introduces, each
    /// for the person whose section holds it.
    fn motifs(&self, ev: &[Event], zs: usize, ze: usize) -> Vec<MotifLine> {
        let doc = super::motifs::Doc {
            chars: self.scan.chars(),
            breaks: self.scan.breaks(),
            folded: &self.folded,
        };
        let named: Vec<(&'static str, usize, usize)> = ev
            .iter()
            .filter_map(|x| match x {
                Event::Cue(c) => match c.cue {
                    Cue::Poste(code)
                        if !super::motifs::refusing(doc.chars, doc.breaks, doc.folded, c.e) =>
                    {
                        Some((code, c.s, c.e))
                    }
                    _ => None,
                },
                Event::Amount(_) => None,
            })
            .collect();
        let heads: Vec<(usize, usize, &'static str)> = named
            .iter()
            .filter(|(_, s, _)| super::motifs::heading(doc.chars, doc.breaks, doc.folded, zs, *s))
            .map(|(c, s, e)| (*s, *e, *c))
            .collect();
        // A label opens a section when it titles one, or when its paragraph
        // prices something. A bare mention under the title of another poste
        // names nothing of its own: the award below belongs to the title.
        let labels: Vec<Label> = named
            .iter()
            .filter_map(|&(code, cs, ce)| {
                if heads.iter().any(|h| h.0 == cs) {
                    return Some((code, cs, ce, None));
                }
                let ps = super::motifs::hard_paragraph_start(doc, cs, zs);
                let pe = super::motifs::hard_paragraph_end(doc, ce, ze);
                let para = self.text(ps, pe);
                if AWARD_MARKS.iter().any(|m| para.contains(m)) {
                    return Some((code, cs, ce, Some(pe)));
                }
                let governed =
                    heads
                        .iter()
                        .rev()
                        .find(|h| h.0 < cs)
                        .is_some_and(|&(hs, he, hc)| {
                            hc != code
                                && cs - hs <= 2 * SECTION
                                && super::motifs::foreign_opener(doc, &heads, hc, he, cs).is_none()
                        });
                (!governed).then_some((code, cs, ce, None))
            })
            .collect();
        let sections = self.sections(zs, ze);
        let mut out: Vec<MotifLine> = Vec::new();
        let mut i = 0;
        while i < labels.len() {
            let (code, cs, ce, para_end) = labels[i];
            let mut j = i + 1;
            while j < labels.len() && labels[j].0 == code {
                j += 1;
            }
            // The run reaches past its last label: a poste named again in
            // its own section is discussed there.
            let end = labels
                .get(j)
                .map_or(ze, |l| l.1)
                .min(labels[j - 1].2 + SECTION);
            let end = super::motifs::foreign_opener(doc, &heads, code, ce, end).unwrap_or(end);
            let end = para_end.map_or(end, |pe| end.min(pe));
            let section = sections.iter().rev().find(|(at, _)| *at <= cs);
            let name = match section {
                Some((_, Section::Relatives)) => {
                    i = j;
                    continue;
                }
                Some((_, Section::Person(name))) => Some(name.clone()),
                Some((_, Section::Group)) | None => None,
            };
            let line = self.awarded(ev, &labels[i..j], end);
            if let Some(line) = line {
                let about = self.text(cs, (ce + 80).min(end));
                let ricochet = section.is_some_and(|(at, _)| {
                    let intro = self.text(*at, (*at + 160).min(cs));
                    RICOCHET_WORDS.iter().any(|w| intro.contains(w))
                }) || RICOCHET_WORDS.iter().any(|w| about.contains(w));
                out.push(MotifLine {
                    name,
                    ricochet,
                    line,
                });
            }
            i = j;
        }
        out
    }

    /// The first amount of the section that the court's own award verb prices,
    /// a party's claim and the recital of the judgment excluded.
    fn awarded(&self, ev: &[Event], run: &[Label], end: usize) -> Option<Line> {
        let (code, cs, ce, _) = run[0];
        let section = self.text(cs, end);
        let mut marks: Vec<(usize, usize)> = Vec::new();
        for mark in AWARD_MARKS {
            for (b, _) in section.match_indices(mark) {
                let at = cs + section[..b].chars().count();
                marks.push((at, at + mark.chars().count()));
            }
        }
        marks.sort_unstable();
        let awards: Vec<Amount> = marks
            .iter()
            .filter_map(|&(at, from)| {
                let before = self.text(from.saturating_sub(45), from);
                // « a été évalué à » recites an earlier evaluation, and a verb
                // in the parties' claims awards nothing.
                let recited = self.text(at.saturating_sub(4), at) == "ete ";
                let pleaded = self.pleaded_end.is_some_and(|m| at < m);
                if recited || pleaded || CLAIM_MARKS.iter().any(|w| before.contains(w)) {
                    return None;
                }
                ev.iter().find_map(|x| match x {
                    Event::Amount(a) if a.start >= from && self.award_gap(from, a.start) => {
                        let cap = (a.end + 40).min(self.folded.len());
                        let rate = self.text(a.end, cap);
                        let lead = self.text(from, a.start);
                        (!UNIT_MARKS
                            .iter()
                            .any(|w| before.contains(w) || rate.contains(w))
                            && !lead.contains("provision"))
                        .then_some(*a)
                    }
                    _ => None,
                })
            })
            .collect();
        let a = *awards.first()?;
        // The run prices the poste several times, by period or by component,
        // and the total is not the first of them.
        if awards.iter().any(|x| euros(x) != euros(&a)) {
            return None;
        }
        // Several amounts between the last mention of the poste before the
        // award and its next mention, and the choice between them is a guess.
        let lo = run
            .iter()
            .rev()
            .find(|l| l.1 <= a.start)
            .map_or(cs, |l| l.1);
        let hi = run.iter().find(|l| l.1 > a.start).map_or(end, |l| l.1);
        let priced = ev
            .iter()
            .filter(|x| matches!(x, Event::Amount(_)) && x.pos() >= lo && x.pos() < hi)
            .count();
        if priced > AMOUNTS_PER_SECTION {
            return None;
        }
        // The label hung on the amount by a hanger word names the poste,
        // whatever label opened the run.
        let hung = ev.iter().find_map(|x| match x {
            Event::Cue(c) if c.s >= a.end && c.s <= a.end + 60 => match c.cue {
                Cue::Poste(code) => {
                    let gap = self.text(a.end, c.s);
                    (HANGERS.iter().any(|h| gap.contains(h))
                        && !gap.contains(['.', ';', ':'])
                        && !self.breaks(a.end, c.s))
                    .then_some((code, c.s, c.e))
                }
                _ => None,
            },
            _ => None,
        });
        let (code, cs, ce) = hung.unwrap_or((code, cs, ce));
        let around = self.text(ce, (ce + 40).min(self.folded.len()));
        Some(Line {
            code: qualified(code, &around)?,
            label: self.scan.verbatim(cs, ce),
            amount: euros(&a),
            field: Field::Alloue,
            rente: None,
            clause: 0,
        })
    }

    /// Between the award verb and the amount: prose only, one clause at most.
    /// The full stop is allowed, the payee being written « M. [D] ».
    fn award_gap(&self, from: usize, to: usize) -> bool {
        to >= from
            && to - from <= 60
            && !self.folded[from..to]
                .iter()
                .any(|c| c.is_ascii_digit() || matches!(c, ';' | ':'))
    }

    /// « FIXE l'indemnisation des préjudices moraux des ayants droit … : 32.600
    /// € au bénéfice de Madame [L] [A], veuve de Monsieur [E] [A] ; … » : one
    /// head of damage shared between named relatives, each figure written
    /// before the name it goes to. Whether it read the clause whole.
    fn affection_list(&mut self, ev: &[Event], s: usize, e: usize) -> bool {
        let Some(first) = ev.iter().position(|x| matches!(x, Event::Amount(_))) else {
            return false;
        };
        let intro = self.text(s, ev[first].pos());
        let Some(head) = AFFECTION_HEADS
            .iter()
            .find_map(|w| intro.find(w).map(|b| (b, *w)))
        else {
            return false;
        };
        // The heirs claiming the deceased's own damages take his postes, not
        // theirs, and a poste the clause names itself prices its own rows.
        if intro.contains("successoral")
            || ev
                .iter()
                .any(|x| matches!(x.cue(), Some(Cue::Poste(_) | Cue::OutOfScope(_))))
        {
            return false;
        }
        let mut rows: Vec<(Amount, usize, usize)> = Vec::new();
        let mut total: Option<Amount> = None;
        for i in first..ev.len() {
            let Event::Amount(a) = ev[i] else { continue };
            let pay = match ev.get(i + 1) {
                Some(Event::Cue(c))
                    if c.cue == Cue::PayTo
                        && self.gap_ok(self.past_currency(&a), c.s, CONNECTORS) =>
                {
                    c.e
                }
                // « Soit un total de 90.500 € » closes the list and prices nobody.
                _ if i > 0
                    && matches!(ev[i - 1].cue(), Some(Cue::Total | Cue::Sum))
                    && self.gap_ok(ev[i - 1].end(), a.start, AFTER_LABEL) =>
                {
                    total = Some(a);
                    continue;
                }
                _ => return false,
            };
            let stop = ev[i + 2..]
                .iter()
                .find(|x| {
                    matches!(x, Event::Amount(_)) || matches!(x.cue(), Some(Cue::Sum | Cue::Total))
                })
                .map_or(e, Event::pos);
            rows.push((a, pay, stop));
        }
        if rows.len() < 2 {
            return false;
        }
        let read: Vec<(String, Option<Lien>, Amount)> = rows
            .iter()
            .filter_map(|(a, pay, stop)| {
                let raw = self.text(*pay, *stop);
                match person(&raw, self.mode) {
                    Person::Single(name) => Some((name, kinship_of(&raw), *a)),
                    _ => None,
                }
            })
            .collect();
        // A row whose payee is not one named person, or a list no apposition
        // qualifies: nothing says the figures repair the relatives' own loss.
        if read.len() != rows.len() || !read.iter().any(|(_, lien, _)| lien.is_some()) {
            return false;
        }
        let hs = s + intro[..head.0].chars().count();
        let label = self.scan.verbatim(hs, hs + head.1.chars().count());
        for (name, lien, a) in read {
            let d = self.draft(&name);
            self.drafts[d].lien = self.drafts[d].lien.or(lien);
            self.drafts[d].ricochet = true;
            self.drafts[d].lines.push(Line {
                code: "PAF",
                label: label.clone(),
                amount: euros(&a),
                field: Field::Alloue,
                rente: None,
                clause: s,
            });
            self.ob(&a, ObKind::Poste("PAF".to_string()), Owner::Draft(d));
        }
        if let Some(t) = total {
            self.ob(&t, ObKind::Hors("intermediaire"), Owner::None);
        }
        true
    }

    fn raise_at(&self, s: usize, e: usize) -> Option<usize> {
        let text = self.text(s, e);
        RAISED
            .iter()
            .filter_map(|w| text.find(w).map(|b| (b, w.chars().count())))
            .min()
            .map(|(b, n)| s + text[..b].chars().count() + n)
    }

    /// « La somme de 8 415 euros que le centre hospitalier a été condamné à
    /// verser à Mme [D] est portée à 10 075 euros » : the appeal rewrites the
    /// judgment instead of condemning again, so the old figure is a recital
    /// and the new one is what the victim finally receives.
    fn raising(&mut self, ev: &[Event], s: usize, r: usize) {
        let head: Vec<Event> = ev.iter().filter(|x| x.pos() < r).copied().collect();
        self.all_hors(&head, "rappel_du_jugement");
        let net = ev.iter().find_map(|x| match x {
            Event::Amount(a) if a.start >= r && self.gap_ok(r, a.start, AFTER_SUM) => Some(*a),
            _ => None,
        });
        let pay = ev
            .iter()
            .filter(|x| x.cue() == Some(Cue::PayTo) && x.end() <= r)
            .map(Event::end)
            .next_back()
            .or_else(|| self.payee_starts(s, r, false).into_iter().next_back());
        let owner = pay.and_then(|p| {
            // « à verser à Mme F...D...par le jugement du 24 avril 2017 » : the
            // judgment that carried the old figure is no part of the name, and
            // the registry glues it to a pseudonym that ends on its dots.
            let raw = self.text(p, r);
            let raw = BY_JUDGMENT
                .iter()
                .filter_map(|w| raw.find(w))
                .min()
                .map_or(raw.clone(), |i| raw[..i].to_string());
            match person(&raw, self.mode) {
                Person::Single(name) => Some((self.draft(&name), false)),
                Person::Heirs(name) => Some((self.draft(&name), true)),
                _ => None,
            }
        });
        let (Some(a), Some((d, heirs))) = (net, owner) else {
            let rest: Vec<Event> = ev.iter().filter(|x| x.pos() >= r).copied().collect();
            return self.hors_each(ev, &rest, "autre");
        };
        self.drafts[d].successoral |= heirs;
        self.ob(&a, ObKind::Net, Owner::Draft(d));
        self.drafts[d].nets.push(euros(&a));
        let rest: Vec<Event> = ev.iter().filter(|x| x.pos() > a.start).copied().collect();
        self.hors_each(ev, &rest, "autre");
    }

    fn fixing(&mut self, ev: &[Event], s: usize, e: usize) {
        if self.affection_list(ev, s, e) {
            return;
        }
        let first_label = ev
            .iter()
            .find(|x| matches!(x.cue(), Some(Cue::Poste(_))))
            .map_or(e, Event::pos);
        let intro = self.text(s, first_label.min(s + 600));
        // « Fixe la créance de la CPAM à … » : the clause prices the organism's claim.
        let claim = CLAIM_OF
            .iter()
            .filter_map(|w| intro.find(w))
            .min()
            .is_some_and(|at| {
                let after: String = intro[at..].chars().take(60).collect();
                ORGANISM_WORDS.iter().any(|o| after.contains(o))
                    && !intro[..at].contains("deduction")
                    && !intro[..at].contains("deduire")
            });
        if claim {
            return self.hors_each(ev, ev, "tiers_payeur");
        }
        let ricochet = RICOCHET_WORDS.iter().any(|w| intro.contains(w));
        let person = if first_label > s + 600 {
            Some(Person::Several)
        } else {
            match fixed_person(&intro, self.mode) {
                None if ricochet || intro.contains("chacun") || intro.contains("epoux") => {
                    Some(Person::Several)
                }
                p => p,
            }
        };
        let share = SHARE_WORDS.iter().any(|w| intro.contains(w));
        let clause = self.text(s, e);
        let clause_tiers =
            self.tiers_payeur || TIERS_PAYEUR_WORDS.iter().any(|w| clause.contains(w));
        // The clause says it is pricing after the reduction: what it writes is
        // the victim's share, not the poste.
        let clause_reduced = AFTER_REDUCTION.iter().any(|w| intro.contains(w))
            || reduction_cue(&self.folded[s..e.min(self.folded.len())]);
        let provision_at: Vec<bool> = ev.iter().map(|x| self.provision_cue(x)).collect();
        let provisional = provision_at.contains(&true);
        let li = self.lists.len();
        let successoral =
            clause.contains("successoral") || matches!(person, Some(Person::Heirs(_)));
        self.lists.push(FixList {
            person,
            ricochet,
            successoral,
            lines: Vec::new(),
            unsure: Vec::new(),
        });
        let owner = Owner::List(li);
        // A row opens at a label and runs to the next one.
        let mut row_of: Vec<Option<(usize, &'static str)>> = Vec::with_capacity(ev.len());
        let mut cur = None;
        for x in ev {
            if let Some(Cue::Poste(code)) = x.cue() {
                cur = Some((x.pos(), code));
            }
            row_of.push(cur);
        }
        let row_amounts = |k: usize| -> (usize, usize) {
            let mut rank = 0;
            let mut count = 0;
            for (m, x) in ev.iter().enumerate() {
                if matches!(x, Event::Amount(_)) && row_of[m] == row_of[k] {
                    count += 1;
                    if m < k {
                        rank += 1;
                    }
                }
            }
            (rank, count)
        };
        // A provision named in the intro qualifies every row; named in a row,
        // that row alone.
        let row_provisional = |k: usize| {
            (0..ev.len())
                .any(|m| provision_at[m] && (row_of[m].is_none() || row_of[m] == row_of[k]))
        };
        let heads: Vec<usize> = ev
            .iter()
            .filter(|x| x.cue() == Some(Cue::OutOfScope("intermediaire")))
            .map(Event::pos)
            .collect();
        let links: Vec<Link> = (0..ev.len())
            .map(|i| match ev[i] {
                Event::Amount(_) => self.link(ev, i, true),
                Event::Cue(_) => Link::None,
            })
            .collect();
        let totals: Vec<bool> = (0..ev.len())
            .map(|i| match ev[i] {
                Event::Amount(a) => {
                    matches!(links[i], Link::None | Link::Global { .. })
                        && i > 0
                        && matches!(ev[i - 1].cue(), Some(Cue::Total | Cue::Sum))
                        && self.gap_ok(ev[i - 1].end(), a.start, AFTER_LABEL)
                }
                Event::Cue(_) => false,
            })
            .collect();
        // A total under a group heading closes that group, unless it is the last
        // one after every heading: the total of the whole.
        let mut persons: Vec<&String> = links
            .iter()
            .filter_map(|l| match l {
                Link::Poste {
                    person: Some(name), ..
                } => Some(name),
                _ => None,
            })
            .collect();
        persons.sort_unstable();
        persons.dedup();
        let several = persons.len() >= 2;
        let group_total = |i: usize| -> bool {
            if several {
                return true;
            }
            if heads.is_empty() {
                return false;
            }
            let pos = ev[i].pos();
            let before = heads.iter().filter(|h| **h < pos).count();
            if before == 0 {
                return heads.len() == 1;
            }
            let Event::Amount(t) = ev[i] else {
                return false;
            };
            let last_head = heads
                .iter()
                .filter(|h| **h < pos)
                .max()
                .copied()
                .unwrap_or(0);
            let sum_since = |from: usize| -> f64 {
                (0..i)
                    .filter(|m| ev[*m].pos() > from && matches!(links[*m], Link::Poste { .. }))
                    .filter_map(|m| match ev[m] {
                        Event::Amount(a) => Some(euros(&a)),
                        Event::Cue(_) => None,
                    })
                    .sum()
            };
            let same = |x: f64, y: f64| (x - y).abs() < 0.011;
            let t = euros(&t);
            if same(t, sum_since(last_head)) {
                return true;
            }
            if same(t, sum_since(0)) {
                return false;
            }
            before < heads.len() || totals[i + 1..].contains(&true) || before == 1
        };
        // A share written before what the row takes off for a third-party payer:
        // the victim gets the first figure, the poste is both.
        let deducted_next = |k: usize| -> Option<usize> {
            let Event::Amount(x) = ev[k] else { return None };
            let n = (k + 1..ev.len()).find(|m| matches!(ev[*m], Event::Amount(_)))?;
            if ev[k + 1..n]
                .iter()
                .any(|e| matches!(e.cue(), Some(Cue::Poste(_))))
            {
                return None;
            }
            let Event::Amount(y) = ev[n] else { return None };
            let between = self.text(x.end, y.start);
            (y.start - x.end <= 70
                && between.contains("deduction")
                && !between.contains("provision")
                && DEDUCTED_OF.iter().any(|w| between.contains(w)))
            .then_some(n)
        };
        let deducted_next: Vec<Option<usize>> = (0..ev.len()).map(deducted_next).collect();
        let deducted_from: Vec<bool> = (0..ev.len())
            .map(|k| deducted_next[..k].contains(&Some(k)))
            .collect();
        // « 11 300 euros au titre du préjudice moral, 5 500 euros au titre des
        // souffrances physiques » : a label written behind its own amount prices
        // that one, and the row it opens does not reach the amounts after it.
        let trailing: Vec<usize> = (0..ev.len())
            .filter_map(|m| match &links[m] {
                Link::Poste { s: ls, .. } if ev[m].pos() < *ls => Some(*ls),
                _ => None,
            })
            .collect();
        let mut linked: Vec<usize> = Vec::new();
        for i in 0..ev.len() {
            let Event::Amount(a) = ev[i] else { continue };
            match &links[i] {
                Link::Poste {
                    code,
                    s: ls,
                    e: le,
                    share: row_share,
                    person,
                } => {
                    let (code, ls, le) = (*code, *ls, *le);
                    // A row of columns prices the poste first, its debtor's share next.
                    let columns = matches!(ev.get(i + 1), Some(Event::Amount(_)));
                    let provisional = if self.mode.extended() {
                        row_provisional(i)
                    } else {
                        provisional
                    };
                    let field = if provisional {
                        Field::None
                    } else if deducted_next[i].is_some() {
                        if self.mode.extended() {
                            Field::Both
                        } else {
                            Field::Pv
                        }
                    } else if (share && !columns) || *row_share {
                        Field::Pv
                    } else if self.mode.extended() {
                        match self.row_mark(ev, i, &a) {
                            RowMark::Tiers => Field::Tiers,
                            RowMark::Victim if columns => Field::Pv,
                            RowMark::Victim => Field::Both,
                            _ if self.component(le, &a) => Field::None,
                            _ if clause_reduced && !columns => Field::Pv,
                            _ if columns || clause_tiers || self.reduced || self.split_reduced => {
                                Field::Alloue
                            }
                            RowMark::Plain => Field::Both,
                        }
                    } else if columns
                        || self.reduced
                        || ((clause_tiers || self.doc_tiers)
                            && !vocabulary::extra_patrimonial(code))
                    {
                        Field::Unknown
                    } else {
                        Field::Alloue
                    };
                    linked.push(ls);
                    let line = Line {
                        code,
                        label: self.scan.verbatim(ls, le),
                        amount: euros(&a),
                        field,
                        rente: self.rente(&a, a.start.saturating_sub(70)),
                        clause: s,
                    };
                    match person {
                        Some(name) if self.mode.extended() => {
                            let d = self.draft(name);
                            self.drafts[d].ricochet |= vocabulary::direct_side(code) == Some(false);
                            self.drafts[d].lines.push(line);
                            self.ob(&a, ObKind::Poste(code.to_string()), Owner::Draft(d));
                        }
                        _ => {
                            self.lists[li].lines.push(line);
                            self.ob(&a, ObKind::Poste(code.to_string()), owner);
                        }
                    }
                }
                Link::Out(cause) => self.ob(&a, ObKind::Hors(cause), Owner::None),
                _ if self.interest_base(&a) => {
                    self.ob(&a, ObKind::Hors("intermediaire"), Owner::None)
                }
                _ if self.solde(ev, i, &a, e) => {
                    self.soldes.push(euros(&a));
                    self.ob(&a, ObKind::Hors("intermediaire"), Owner::None)
                }
                _ if totals[i] => {
                    if group_total(i) {
                        self.ob(&a, ObKind::Hors("intermediaire"), Owner::None)
                    } else {
                        self.ob(&a, ObKind::Total, owner)
                    }
                }
                Link::Global { .. } | Link::None => {
                    let row = row_of[i].filter(|(ls, _)| !trailing.contains(ls));
                    let Some((_, code)) = row else {
                        self.ob(&a, ObKind::Indemnite, owner);
                        continue;
                    };
                    if self.deducted_of_tiers(ev, i, &a) {
                        self.ob(&a, ObKind::Hors("tiers_payeur"), Owner::None);
                        continue;
                    }
                    if self.foreign_label(ev, i, &a) {
                        self.ob(&a, ObKind::Indemnite, owner);
                        continue;
                    }
                    if let Some(rente) = self.rente(&a, a.start.saturating_sub(70)) {
                        if self.mode.extended() {
                            self.lists[li].lines.push(Line {
                                code,
                                label: self.scan.verbatim(a.start, a.end),
                                amount: euros(&a),
                                field: Field::None,
                                rente: Some(rente),
                                clause: s,
                            });
                        }
                        self.ob(&a, ObKind::Poste(code.to_string()), owner);
                        continue;
                    }
                    let (rank, count) = row_amounts(i);
                    let mark = self.row_mark(ev, i, &a);
                    if deducted_from[i] {
                        if self.mode.extended() {
                            self.lists[li].lines.push(Line {
                                code,
                                label: self.scan.verbatim(a.start, a.end),
                                amount: euros(&a),
                                field: Field::Alloue,
                                rente: None,
                                clause: s,
                            });
                        }
                        self.ob(&a, ObKind::Poste(code.to_string()), owner);
                        continue;
                    }
                    if rank >= 1 && count >= 3 && mark == RowMark::Plain {
                        self.ob(&a, ObKind::Hors("intermediaire"), Owner::None);
                        continue;
                    }
                    if mark == RowMark::Victim && self.mode.extended() {
                        self.lists[li].lines.push(Line {
                            code,
                            label: self.scan.verbatim(a.start, a.end),
                            amount: euros(&a),
                            field: Field::Pv,
                            rente: None,
                            clause: s,
                        });
                    }
                    self.ob(&a, ObKind::Poste(code.to_string()), owner);
                }
            }
        }
        let unsure = self.unsure_codes(ev, s, e, &linked);
        self.lists[li].unsure = unsure.into_iter().map(|c| (c, s)).collect();
    }
}

/// A gap of prose alone: no digit, few words, none of them a payment verb or
/// a sum word.
/// Three dots or an ellipsis: the leaders of a tabulated line, which fill the
/// space between a label and the figure it prices.
fn leaders(gap: &str) -> bool {
    gap.contains("...") || gap.contains('\u{2026}')
}

/// « euros de souffrances endurées », « 2 000 € titre des souffrances endurées » :
/// the preposition alone between a sum and its poste.
fn de_led(gap: &str) -> bool {
    matches!(
        gap.trim(),
        "de" | "d'" | "du" | "des" | "de la" | "de l'" | "titre de" | "titre du" | "titre des"
    )
}

/// The text with what stands in brackets taken out: « 7.840€ (sept mille huit
/// cent quarante euros) au titre du » spells its figure between the sum and the
/// label.
fn unparenthesised(s: &str) -> String {
    let mut depth = 0usize;
    s.chars()
        .filter(|c| {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            depth == 0 && *c != ')'
        })
        .collect()
}

/// Does the lead end on a bullet, which opens an item of a list?
fn bullet_led(lead: &str) -> bool {
    let t = lead.trim_end();
    let Some(last) = t.chars().next_back() else {
        return false;
    };
    BULLETS.contains(&last)
        && t[..t.len() - last.len_utf8()]
            .chars()
            .next_back()
            .is_none_or(|c| matches!(c, ' ' | ':' | ';' | ','))
}

fn prose_only(gap: &str) -> bool {
    const HEAVY: [&str; 7] = [
        "payer", "verser", "regler", "condamn", "soit", "allou", "somme",
    ];
    !gap.chars().any(|c| c.is_ascii_digit())
        && gap.split_whitespace().count() <= 8
        && !HEAVY.iter().any(|w| gap.contains(w))
}

/// `Some(true)` direct, `Some(false)` ricochet, `None` when neither the codes
/// nor the wording settle it.
fn role_of<'a>(
    codes: impl Iterator<Item = &'a str>,
    ricochet: bool,
    successoral: bool,
) -> Option<bool> {
    let votes: Vec<bool> = codes.filter_map(vocabulary::direct_side).collect();
    if votes.iter().any(|v| *v) || successoral {
        Some(true)
    } else if !votes.is_empty() || ricochet {
        Some(false)
    } else {
        None
    }
}

/// A poste the motifs price is written where the dispositif left the field
/// empty, and opened when the dispositif never named it.
fn fill_from_motifs(postes: &mut Vec<Poste>, code: &str, line: &Line) {
    match postes.iter_mut().find(|p| p.poste.as_deref() == Some(code)) {
        Some(p) if p.montant_alloue.is_none() => p.montant_alloue = Some(line.amount),
        Some(_) => {}
        None => postes.push(Poste {
            poste: Some(code.to_string()),
            libelle_decision: Some(line.label.to_lowercase()),
            montant_alloue: Some(line.amount),
            ..Default::default()
        }),
    }
}

/// The record and the obligations the dispositif alone fixes.
pub fn extract(scan: &DocScan, mode: Mode) -> Extraction {
    let (ds, de) = super::dispositif_span(scan);
    if ds >= de {
        return Extraction::default();
    }
    let folded: Vec<char> = scan.folded().chars().collect();
    let chars = scan.chars();
    let mut cues: Vec<CueTok> =
        vocabulary::cues_squeezed(scan.folded(), scan.byte2char(), mode.extended())
            .into_iter()
            .filter(|c| c.s >= ds && c.e <= de)
            .collect();
    let abbreviated: Vec<CueTok> = vocabulary::abbreviations(chars)
        .into_iter()
        .filter(|a| a.s >= ds && a.e <= de && !cues.iter().any(|c| a.s < c.e && c.s < a.e))
        .collect();
    cues.extend(abbreviated);
    cues.sort_unstable_by_key(|c| (c.s, c.e));
    let mut amounts: Vec<Amount> = leftovers::amounts_in(chars, &folded)
        .into_iter()
        .filter(|a| a.start >= ds && a.end <= de)
        .collect();
    for c in cues.iter().filter(|c| matches!(c.cue, Cue::Poste(_))) {
        if let Some((s, e)) = zero_after(&folded, c.e, de) {
            if !amounts.iter().any(|a| a.start == s) {
                amounts.push(Amount {
                    value: 0,
                    start: s,
                    end: e,
                });
            }
        }
    }
    // The Pacific franc prices the judgments of Papeete and Nouméa; the table
    // converts it at its fixed parity, and only if the dispositif names it. A
    // Polynesian hospital is condemned in euros over claims recited in francs,
    // and what the record keeps is what fixes its currency.
    let currencies = |from: usize| -> Vec<&'static str> {
        amounts
            .iter()
            .filter(|a| a.start >= from)
            .filter_map(|a| leftovers::currency_word(&folded, a.end))
            .collect()
    };
    let named = match currencies(ds) {
        v if v.is_empty() => currencies(0),
        v => v,
    };
    // « les frais d'expertise taxés à la somme de 304 500 F CFP » sits in a
    // dispositif that condemns in euros: one article in francs does not price
    // the arrêt, the currency most of its articles are written in does.
    let francs = named.iter().filter(|w| leftovers::is_cfp(w)).count();
    let cfp = francs * 2 > named.len();
    let mut events: Vec<Event> = amounts
        .into_iter()
        .map(Event::Amount)
        .chain(cues.iter().copied().map(Event::Cue))
        .collect();
    events.sort_by_key(Event::pos);

    let futures: Vec<usize> = (ds..de.min(folded.len()))
        .filter(|i| {
            (*i == 0 || !folded[i - 1].is_alphanumeric())
                && FUTURE_PAYS.contains(&word_at(&folded, *i).as_str())
        })
        .collect();
    let renewals: Vec<usize> = (ds..de.min(folded.len()))
        .filter(|i| {
            (*i == 0 || !folded[i - 1].is_alphabetic())
                && RENEWED.iter().any(|w| leftovers::word_at(&folded, *i, w))
        })
        .collect();
    let mut heads: Vec<(usize, Kind)> = scan
        .clause_starts_in(ds, de)
        .into_iter()
        .chain(futures)
        .chain(
            cues.iter()
                .filter(|c| {
                    matches!(c.cue, Cue::Condemn | Cue::Fix | Cue::Reject)
                        || (c.cue == Cue::PayTo
                            && ALLOCATES.iter().any(|v| word_at(&folded, c.s) == *v))
                })
                .map(|c| c.s),
        )
        .filter_map(|p| head_kind(chars, &folded, p, mode).map(|k| (p, k)))
        .chain(renewals.iter().map(|p| (*p, Kind::Other)))
        // The administrative order cuts its dispositif into numbered articles,
        // and one article does not pay for the next: « Article 3 : Les frais
        // d'expertise […] sont mis à la charge » otherwise makes costs of the
        // sum article 1 awards.
        .chain(
            (ds + 1..de.min(folded.len()))
                .filter(|i| article_head(chars, &folded, *i))
                .map(|p| (p, Kind::Other)),
        )
        .collect();
    heads.push((ds, Kind::Other));
    heads.sort_by_key(|h| h.0);
    heads.dedup_by_key(|h| h.0);

    let dispositif: String = folded[ds..de].iter().collect();
    let doc_tiers = TIERS_PAYEUR_WORDS
        .iter()
        .chain(&["indemnites journalieres", "rente"])
        .any(|w| scan.folded().contains(w));
    let pleaded_end = super::motifs::discussion_start(
        chars,
        scan.breaks(),
        &folded,
        scan.motifs_start().min(ds),
        ds,
    );
    let mut cp = Composer {
        scan,
        mode,
        folded,
        reduced: false,
        drafts: Vec::new(),
        lists: Vec::new(),
        obs: Vec::new(),
        tiers_payeur: TIERS_PAYEUR_WORDS.iter().any(|w| dispositif.contains(w)),
        doc_tiers,
        provisioned: dispositif.contains("provision"),
        split_reduced: AFTER_REDUCTION.iter().any(|w| dispositif.contains(w)),
        confirmed: Vec::new(),
        companions: Vec::new(),
        clause_start: 0,
        pleaded_end,
        confirms: false,
        soldes: Vec::new(),
        rentes: Vec::new(),
        lone_nets: Vec::new(),
        condemns: Vec::new(),
    };
    cp.reduced = reduction_cue(&cp.folded);
    let header_end = scan.motifs_start().min(ds);
    let header: Vec<Event> = leftovers::amounts_in(chars, &cp.folded)
        .into_iter()
        .filter(|a| a.end <= header_end)
        .map(Event::Amount)
        .chain(
            vocabulary::cues_squeezed(scan.folded(), scan.byte2char(), mode.extended())
                .into_iter()
                .filter(|c| c.e <= header_end)
                .map(Event::Cue),
        )
        .collect();
    let mut header = header;
    header.sort_by_key(Event::pos);
    let header_codes: Vec<&'static str> = (0..header.len())
        .filter_map(|i| match cp.link(&header, i, true) {
            Link::Poste { code, .. } => Some(code),
            _ => None,
        })
        .collect();
    let mut reversed = false;
    let mut recited = false;
    for (n, &(s, kind)) in heads.iter().enumerate() {
        let e = heads.get(n + 1).map_or(de, |h| h.0);
        recited = match kind {
            _ if renewals.contains(&s) => false,
            Kind::Recital => true,
            Kind::Other => recited,
            _ => false,
        };
        cp.clause_start = s;
        let clause: Vec<Event> = events
            .iter()
            .filter(|ev| ev.pos() >= s && ev.pos() < e)
            .copied()
            .collect();
        // A condemnation written inside another chef pays on its own.
        let split = (!matches!(kind, Kind::Condemn))
            .then(|| {
                clause.iter().position(|x| {
                    x.cue() == Some(Cue::Condemn)
                        && x.pos() > s
                        && cp.text(x.pos(), x.end()) == "condamne"
                        && chars[x.end() - 1] != 'é'
                        && (kind != Kind::Recital
                            || RENEWED.iter().any(|w| cp.text(s, x.pos()).contains(w)))
                        && !RECITAL_BEFORE
                            .iter()
                            .any(|w| cp.text(x.pos().saturating_sub(40), x.pos()).contains(w))
                })
            })
            .flatten();
        let split = split.filter(|k| clause[*k..].iter().any(|x| x.cue() == Some(Cue::PayTo)));
        let (clause, tail) = match split {
            Some(k) => (clause[..k].to_vec(), Some(clause[k..].to_vec())),
            None => (clause, None),
        };
        let whole_end = e;
        let e = tail.as_ref().map_or(e, |t| t[0].pos());
        // « Rappelle » recaps the court's own figures, unless it recites the judgment.
        let recap = cp.text(s, s + 8) == "rappelle";
        let recites = recap && RECITED.iter().any(|w| cp.text(s, e).contains(w));
        match kind {
            Kind::Condemn => cp.payment(&clause, s, e),
            Kind::Fix if recites => cp.all_hors(&clause, "rappel_du_jugement"),
            Kind::Fix => cp.fixing(&clause, s, e),
            Kind::Reject => cp.hors_each(&clause, &clause, "pretention"),
            Kind::Recital => {
                let text = cp.text(s, e);
                let keeps = if text.starts_with("sauf") {
                    reversed
                } else {
                    reversed = !text.starts_with("confirme");
                    !reversed || text.contains("sauf en ce qu")
                };
                if keeps {
                    cp.confirmation(&clause, s, e, chars)
                } else {
                    cp.all_hors(&clause, "rappel_du_jugement")
                }
                // « Confirme le jugement sauf : » then its chiefs one per line:
                // an exception left open turns the lines that follow around.
                let exception = OPEN_EXCEPTIONS
                    .iter()
                    .filter_map(|w| text.rfind(w))
                    .filter(|x| *x > 0)
                    .max();
                if exception.is_some_and(|x| text.rfind("en ce qu").is_none_or(|y| y < x)) {
                    reversed = !reversed;
                }
            }
            Kind::Other if !recited && cp.raise_at(s, e).is_some() => {
                let r = cp.raise_at(s, e).unwrap_or(s);
                cp.raising(&clause, s, r);
            }
            // « Confirme le jugement | en ce qu'il a : … » cut at a line break.
            Kind::Other
                if recited
                    && cp
                        .text(s, (s + 12).min(e))
                        .trim_start_matches(|c: char| !c.is_alphabetic())
                        .starts_with("en ce qu") =>
            {
                if reversed {
                    cp.all_hors(&clause, "rappel_du_jugement")
                } else {
                    cp.confirmation(&clause, s, e, chars)
                }
            }
            // A listing intro, « comme suit », liquidates without a fixing verb.
            Kind::Other
                if LISTING.iter().any(|w| cp.text(s, e).contains(w))
                    && !RECITED.iter().any(|w| cp.text(s, e).contains(w))
                    && clause
                        .iter()
                        .any(|x| matches!(x.cue(), Some(Cue::Poste(_)))) =>
            {
                cp.fixing(&clause, s, e)
            }
            Kind::Other => {
                let text = cp.text(s, e);
                cp.confirms |= text.contains("confirme") && text.contains("surplus");
                cp.hors_each(&clause, &clause, "autre")
            }
        }
        if let Some(tail) = tail {
            cp.payment(&tail, tail[0].pos(), whole_end);
        }
    }
    // A condemnation equal to a balance the dispositif computed after the
    // provisions is that balance.
    let soldes: Vec<f64> = std::mem::take(&mut cp.soldes);
    let same = |x: f64, y: f64| (x - y).abs() < 0.005;
    for o in &mut cp.obs {
        if matches!(o.kind, ObKind::Indemnite | ObKind::Net | ObKind::Total)
            && soldes.iter().any(|s| same(*s, o.montant))
        {
            if let (ObKind::Net, Owner::Draft(d)) = (&o.kind, o.owner) {
                cp.drafts[d].nets.retain(|n| !same(*n, o.montant));
            }
            o.kind = ObKind::Hors("intermediaire");
        }
    }

    // « condamnée à verser à Mme [A] la somme de 8 527 euros », under a
    // liability shared in half: what the reduction takes is a share of a poste,
    // and a dispositif that prices none leaves its lone sum one reading. The
    // administrative order liquidates that way, and its share is always taken.
    if cp.reduced
        && cp.lists.is_empty()
        && !cp.obs.iter().any(|o| matches!(o.kind, ObKind::Poste(_)))
    {
        for d in 0..cp.drafts.len() {
            let own: Vec<usize> = cp
                .obs
                .iter()
                .enumerate()
                .filter(|(_, o)| o.owner == Owner::Draft(d) && !matches!(o.kind, ObKind::Hors(_)))
                .map(|(k, _)| k)
                .collect();
            let [k] = own[..] else { continue };
            if matches!(cp.obs[k].kind, ObKind::Indemnite) && !cp.rentes.contains(&cp.obs[k].s) {
                cp.obs[k].kind = ObKind::Net;
                cp.drafts[d].nets.push(cp.obs[k].montant);
            }
        }
    }

    let from_motifs = {
        let zs = scan.motifs_start().min(ds);
        let mut zone: Vec<Event> = leftovers::amounts_in(chars, &cp.folded)
            .into_iter()
            .filter(|a| a.start >= zs && a.end <= ds)
            .map(Event::Amount)
            .chain(
                vocabulary::cues_squeezed(scan.folded(), scan.byte2char(), mode.extended())
                    .into_iter()
                    .filter(|c| c.s >= zs && c.e <= ds)
                    .map(Event::Cue),
            )
            .collect();
        zone.sort_by_key(Event::pos);
        cp.motifs(&zone, zs, ds)
    };

    if cp.confirms {
        cp.confirmed.extend(header_codes);
    }
    let names: Vec<Option<String>> = cp
        .lists
        .iter()
        .map(|l| match &l.person {
            Some(Person::Single(name) | Person::Heirs(name)) => Some(name.clone()),
            _ => None,
        })
        .collect();
    let mut list_draft: Vec<Option<usize>> = names
        .iter()
        .map(|n| n.as_deref().map(|name| cp.draft(name)))
        .collect();
    let unnamed: Vec<usize> = (0..cp.lists.len())
        .filter(|li| cp.lists[*li].person.is_none() && !cp.lists[*li].lines.is_empty())
        .collect();
    // Several unnamed lists price one person when they are all the direct
    // victim's and no poste appears in two of them.
    let one_victim = unnamed.iter().all(|li| {
        let l = &cp.lists[*li];
        role_of(l.lines.iter().map(|x| x.code), l.ricochet, l.successoral) == Some(true)
    }) && {
        let mut codes: Vec<&str> = Vec::new();
        unnamed.iter().all(|li| {
            let mut own: Vec<&str> = cp.lists[*li].lines.iter().map(|x| x.code).collect();
            own.sort_unstable();
            own.dedup();
            let fresh = own.iter().all(|c| !codes.contains(c));
            codes.extend(own);
            fresh
        })
    };
    for li in unnamed.iter().copied() {
        if cp.drafts.len() == 1 {
            list_draft[li] = Some(0);
        } else if cp.drafts.is_empty() && (unnamed.len() == 1 || one_victim || mode.extended()) {
            list_draft[li] = Some(cp.draft(""));
        }
    }
    for (li, target) in list_draft.iter().enumerate() {
        let Some(d) = *target else { continue };
        let list = &mut cp.lists[li];
        let lines = std::mem::take(&mut list.lines);
        let unsure = std::mem::take(&mut list.unsure);
        cp.drafts[d].ricochet |= list.ricochet;
        cp.drafts[d].successoral |= list.successoral;
        cp.drafts[d].lines.extend(lines);
        cp.drafts[d].unsure.extend(unsure);
    }
    // A sum read as the whole a person receives is not, when another clause
    // pays or fixes something else for the same person.
    for (k, s, e) in std::mem::take(&mut cp.lone_nets) {
        let Owner::Draft(d) = cp.obs[k].owner else {
            continue;
        };
        let elsewhere = cp.obs.iter().any(|o| {
            let same = match o.owner {
                Owner::Draft(x) => x == d,
                Owner::List(li) => list_draft[li] == Some(d),
                Owner::None => false,
            };
            same && (o.s < s || o.s >= e)
                && matches!(o.kind, ObKind::Poste(_) | ObKind::Indemnite | ObKind::Net)
        });
        if elsewhere {
            let m = cp.obs[k].montant;
            cp.obs[k].kind = ObKind::Indemnite;
            cp.drafts[d].nets.retain(|n| (*n - m).abs() >= 0.005);
        }
    }
    // What the motifs price under someone's name goes to that person; the rest
    // waits for the single direct victim.
    let mut unnamed: Vec<Line> = Vec::new();
    for ml in from_motifs {
        match ml.name {
            // « Sur les préjudices de Mme B F » names whose postes the section
            // prices. The sure mode reads it to know what each payee of the
            // dispositif is a victim of, and opens no victim on it: a name the
            // dispositif never paid received nothing that is written.
            Some(name) => {
                let d = match mode.extended() {
                    true => Some(cp.draft(&name)),
                    false => cp.known(&name),
                };
                let Some(d) = d else { continue };
                cp.drafts[d].ricochet |= ml.ricochet;
                cp.drafts[d].motifs.push(ml.line);
            }
            None if mode.extended() && !ml.ricochet => unnamed.push(ml.line),
            None => {}
        }
    }

    let mut victimes = Vec::new();
    let mut draft_cle: Vec<Option<String>> = vec![None; cp.drafts.len()];
    let roles: Vec<Option<bool>> = cp
        .drafts
        .iter()
        .map(|d| {
            let codes = d.lines.iter().chain(&d.motifs).map(|l| l.code);
            role_of(codes, d.ricochet, d.successoral)
        })
        .collect();
    // A payee nothing qualifies is the direct victim, unless the postes already
    // name one: then a lump sum to someone else repairs a ricochet.
    let voted_direct = roles.contains(&Some(true));
    // Two payees, or a text that names an indirect victim, and nothing says
    // which of them the lone sum repairs.
    let sole_payee =
        cp.drafts.len() == 1 && !RICOCHET_NAMED.iter().any(|w| scan.folded().contains(w));
    for (i, d) in cp.drafts.iter().enumerate() {
        // A payee the dispositif pays in one sum, under no poste: the
        // administrative order liquidates that way, and she is the victim when
        // no other payee is voted one.
        let served = cp
            .obs
            .iter()
            .any(|o| o.owner == Owner::Draft(i) && cp.rentes.contains(&o.s));
        let lone = (!d.nets.is_empty() || served) && !voted_direct && sole_payee;
        // « à Mme [U] et à M. [Z] la somme de 10 000 euros à chacun en
        // réparation de leur préjudice moral » : the postes already name a
        // direct victim, so the moral prejudice of another payee is affection.
        let affection = voted_direct && d.lines.iter().chain(&d.motifs).any(|l| l.code == "MORAL");
        let Some(directe) = roles[i]
            .or_else(|| affection.then_some(false))
            .or_else(|| (lone || mode.extended()).then_some(!voted_direct))
        else {
            continue;
        };
        let mut seen: Vec<(&str, f64, usize)> = Vec::new();
        let switched = |code: &'static str| -> Option<&'static str> {
            match (code, directe) {
                ("FD", false) => Some("FDV"),
                ("PS", false) => Some("PEX"),
                ("PR", true) => None,
                // « Préjudice moral » votes for neither side: a relative's is the
                // affection, a direct victim's is whatever the court means by it.
                ("MORAL", false) => Some("PAF"),
                ("MORAL" | "PEX", true) => None,
                (c, false) if vocabulary::direct_side(c) == Some(true) => None,
                (c, true) if vocabulary::direct_side(c) == Some(false) => None,
                (c, _) => Some(c),
            }
        };
        let mut postes: Vec<Poste> = Vec::new();
        let mut unknown: Vec<&str> = Vec::new();
        let mut fields: Vec<(&str, Field)> = Vec::new();
        for line in &d.lines {
            let Some(code) = switched(line.code) else {
                continue;
            };
            // The same figure on the same code in another clause repeats it,
            // unless two debtors are each condemned to it.
            let debtor = |cl: usize| cp.condemns.iter().find(|(s, _)| *s == cl).map(|(_, h)| h);
            let repeated = seen.iter().any(|(c, m, cl)| {
                *c == code
                    && *m == line.amount
                    && *cl != line.clause
                    && !matches!((debtor(*cl), debtor(line.clause)), (Some(a), Some(b)) if a != b)
            });
            if repeated {
                continue;
            }
            seen.push((code, line.amount, line.clause));
            let unsettled = |c: &str| {
                d.unsure.iter().any(|(u, cl)| *u == c && *cl == line.clause)
                    || (!mode.extended() && cp.confirmed.contains(&c))
            };
            let field = if unsettled(line.code) || unsettled(code) {
                Field::Unknown
            } else {
                line.field
            };
            if field == Field::Unknown {
                unknown.push(code);
            }
            fields.push((code, field));
            let p = match postes.iter().position(|p| p.poste.as_deref() == Some(code)) {
                Some(k) => &mut postes[k],
                None => {
                    postes.push(Poste {
                        poste: Some(code.to_string()),
                        ..Default::default()
                    });
                    postes.last_mut().expect("just pushed")
                }
            };
            let label = line.label.to_lowercase();
            p.libelle_decision = Some(match &p.libelle_decision {
                Some(l) if l.contains(&label) => l.clone(),
                Some(l) => format!("{l} ; {label}"),
                None => label,
            });
            if let Some(period) = line.rente {
                p.composantes.push(Composante {
                    libelle: Some(line.label.to_lowercase()),
                    rente: Some(Rente {
                        montant: Some(line.amount),
                        periodicite: period.map(str::to_string),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                continue;
            }
            if matches!(field, Field::Pv | Field::Both) {
                p.part_victime = Some(p.part_victime.unwrap_or(0.0) + line.amount);
            }
            if field == Field::Tiers {
                // A reduced record: the creance is what remains due, not the poste.
                let due = if cp.reduced {
                    &mut p.montant_du
                } else {
                    &mut p.montant_alloue
                };
                *due = Some(due.unwrap_or(0.0) + line.amount);
                p.part_tiers_payeurs = Some(p.part_tiers_payeurs.unwrap_or(0.0) + line.amount);
            }
            if matches!(field, Field::Alloue | Field::Both) {
                p.montant_alloue = Some(p.montant_alloue.unwrap_or(0.0) + line.amount);
                p.composantes.push(Composante {
                    libelle: Some(line.label.to_lowercase()),
                    montant: Some(line.amount),
                    ..Default::default()
                });
            }
        }
        // One line prices the poste, another its share: the two figures are not
        // one poste, and the sure mode writes neither.
        if !mode.extended() {
            for (code, _) in &fields {
                let has = |f: Field| fields.iter().any(|(c, g)| c == code && *g == f);
                if has(Field::Pv) && has(Field::Alloue) && !unknown.contains(code) {
                    unknown.push(code);
                }
            }
        }
        for p in &mut postes {
            if p.poste.as_deref().is_some_and(|c| unknown.contains(&c)) {
                p.part_victime = None;
                p.montant_alloue = None;
                p.part_tiers_payeurs = None;
                p.composantes.retain(|c| c.rente.is_some());
            }
            if p.composantes.len() < 2 && p.composantes.iter().all(|c| c.rente.is_none()) {
                p.composantes.clear();
            }
            if let Some(v) = p.part_victime {
                p.part_victime = Some((v * 100.0).round() / 100.0);
            }
            if let Some(v) = p.montant_alloue {
                p.montant_alloue = Some((v * 100.0).round() / 100.0);
            }
            if let Some(v) = p.montant_du {
                p.montant_du = Some((v * 100.0).round() / 100.0);
            }
        }
        // The sure mode votes the role on those lines and prices nothing with
        // them: a figure the dispositif does not carry is not proven.
        for line in d.motifs.iter().filter(|_| mode.extended()) {
            let Some(code) = switched(line.code) else {
                continue;
            };
            if directe && vocabulary::direct_side(code) == Some(false) {
                continue;
            }
            // In a reduced record, the motifs figure may be the share the
            // dispositif already wrote: nothing is filled on an existing poste.
            if cp.reduced && postes.iter().any(|p| p.poste.as_deref() == Some(code)) {
                continue;
            }
            fill_from_motifs(&mut postes, code, line);
        }
        let net_victime = match d.nets.as_slice() {
            [n] => Some(*n),
            _ => None,
        };
        if postes.is_empty() && net_victime.is_none() && !served {
            continue;
        }
        let cle = format!("v{}", victimes.len() + 1);
        draft_cle[i] = Some(cle.clone());
        victimes.push(Victime {
            cle,
            role: if directe { "directe" } else { "indirecte" }.to_string(),
            lien: (!directe).then_some(d.lien).flatten(),
            net_victime,
            postes,
            ..Default::default()
        });
    }

    if !unnamed.is_empty() {
        let directs: Vec<usize> = (0..victimes.len())
            .filter(|k| victimes[*k].role == "directe")
            .collect();
        let target = match (directs.as_slice(), victimes.is_empty()) {
            ([k], _) => Some(*k),
            (_, true) => {
                victimes.push(Victime {
                    cle: "v1".to_string(),
                    role: "directe".to_string(),
                    ..Default::default()
                });
                Some(0)
            }
            _ => None,
        };
        if let Some(k) = target {
            for line in &unnamed {
                let held = victimes[k]
                    .postes
                    .iter()
                    .any(|p| p.poste.as_deref() == Some(line.code));
                if vocabulary::direct_side(line.code) == Some(false) || (cp.reduced && held) {
                    continue;
                }
                fill_from_motifs(&mut victimes[k].postes, line.code, line);
            }
        }
    }

    let cle_of = |owner: Owner| -> Option<String> {
        match owner {
            Owner::Draft(d) => draft_cle[d].clone(),
            Owner::List(li) => list_draft[li].and_then(|d| draft_cle[d].clone()),
            Owner::None => None,
        }
    };
    let mut obligations: Vec<Obligation> = cp
        .obs
        .iter()
        .filter_map(|o| {
            let cle = cle_of(o.owner);
            let nature = match &o.kind {
                ObKind::Poste(code) => Nature::Poste {
                    cle,
                    code: code.clone(),
                },
                ObKind::Indemnite => Nature::Indemnite { cle },
                ObKind::Net => Nature::Net { cle: cle? },
                ObKind::Total => Nature::Total { cle },
                ObKind::Hors(c) => Nature::HorsRecord(c),
            };
            Some(Obligation {
                montant: o.montant,
                s: o.s,
                e: o.e,
                nature,
            })
        })
        .collect();
    for ev in &events {
        let Event::Amount(a) = ev else { continue };
        if !obligations.iter().any(|o| o.s == a.start) {
            obligations.push(Obligation {
                montant: euros(a),
                s: a.start,
                e: a.end,
                nature: Nature::HorsRecord("autre"),
            });
        }
    }
    obligations.sort_by_key(|o| o.s);
    let mut x = Extraction {
        quantum: Quantum {
            est_liquidation: !victimes.is_empty(),
            devise: Some(if cfp { "XPF" } else { "EUR" }.to_string()),
            lecture: Some(Lecture {
                agent: Some(mode.agent().to_string()),
                passes: 1,
                ..Default::default()
            }),
            victimes,
            ..Default::default()
        },
        obligations,
    };
    super::tables::apply(scan, &mut x, mode);
    super::motifs::apply(scan, &mut x, mode);
    super::cofactors::apply(scan, &mut x, mode);
    x.quantum.est_liquidation = !x.quantum.victimes.is_empty();
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_extract::scan::scan;

    const TJ: &str = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS DE LA DECISION\nSur les demandes.\nPAR CES MOTIFS\nLe tribunal,\nCONDAMNE in solidum les sociétés CARDIF IARD et AREAS DOMMAGES à payer à Monsieur [A] [W] la somme de 38 017,83 euros, en deniers ou quittances, en réparation de ses préjudices personnels subis en sa qualité de victime par ricochet, dont 13 017,83 euros au titre de sa perte de revenus et 25 000 euros au titre de son préjudice d'affection ;\nCONDAMNE in solidum les sociétés CARDIF IARD et AREAS DOMMAGES à payer aux demandeurs la somme de 15 000 euros au titre de l'article 700 du code de procédure civile ;\nCONDAMNE les défenderesses aux dépens.";

    fn postes(v: &Victime) -> Vec<(&str, Option<f64>, Option<f64>)> {
        v.postes
            .iter()
            .map(|p| {
                (
                    p.poste.as_deref().unwrap(),
                    p.montant_alloue,
                    p.part_victime,
                )
            })
            .collect()
    }

    /// The administrative order writes its dispositif in articles, condemns in
    /// the passive present or the future indicative, names its payee after the
    /// money, and pseudonymises by initials.
    const TA: &str = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\nQuant aux souffrances endurées :\n12. Il résulte de l'instruction que Mme A a enduré des souffrances évaluées par l'expert à 4 sur une échelle de 7. Il sera fait une juste appréciation de ce chef de préjudice en lui allouant la somme de 12 000 euros.\nQuant au déficit fonctionnel permanent :\n13. Il y a lieu de mettre à la charge du centre hospitalier le versement à l'intéressée de la somme de 30 000 euros au titre de son déficit fonctionnel permanent.\nD E C I D E :\nArticle 1er : Le centre hospitalier est condamné à verser une somme de 42 000 euros à Mme D A en réparation de ses préjudices.\nArticle 2 : Le centre hospitalier versera à Mme D A la somme de 1 500 euros au titre de l'article L. 761-1 du code de justice administrative.";

    #[test]
    fn an_article_that_pays_after_the_money_opens_the_victim_and_its_motifs() {
        let x = extract(&scan(TA), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].role, "directe");
        assert_eq!(v[0].net_victime, Some(42000.0));
        assert_eq!(
            postes(&v[0]),
            vec![("SE", Some(12000.0), None), ("DFP", Some(30000.0), None)]
        );
        assert!(x
            .obligations
            .iter()
            .any(|o| o.montant == 1500.0 && o.nature == Nature::HorsRecord("article_700")));
    }

    #[test]
    fn an_article_of_the_dispositif_does_not_pay_for_the_next() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation du préjudice.\nDÉCIDE :\nArticle 1er : Le CIVEN est condamné à verser à M. D A la somme de 61 583 euros.\nArticle 2 : Le surplus des conclusions de la requête est rejeté.\nArticle 3 : Les honoraires et frais d'expertise, liquidés et taxés à la somme de 2 700 euros, sont mis à la charge définitive du CIVEN.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].net_victime, Some(61583.0));
        assert!(x
            .obligations
            .iter()
            .any(|o| o.montant == 2700.0 && o.nature == Nature::HorsRecord("depens")));
    }

    #[test]
    fn a_heading_that_opens_another_person_closes_the_section_above_it() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nS'agissant du préjudice d'agrément :\n18. Mme B a dû cesser la course à pied. Il y a lieu de lui allouer la somme de 2 000 euros.\nEn ce qui concerne le préjudice de M. A :\n19. M. H A, père de la requérante, sollicite l'indemnisation des frais de transport. Il sera fait une juste appréciation de son préjudice en lui allouant la somme de 1 666 euros.\nD E C I D E :\nArticle 1er : Le centre hospitalier est condamné à verser à Mme E B la somme de 2 000 euros.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("PA", Some(2000.0), None)]);
    }

    #[test]
    fn a_provision_to_be_deducted_prices_the_list_above_it() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nEvalue les préjudices corporels de Mme [F] [V], hors débours de la CPAM, ainsi qu'il suit :\n- déficit fonctionnel temporaire 691 euros\n- souffrances endurées 4 000 euros\n- préjudice esthétique temporaire 150 euros\n- déficit fonctionnel permanent 3 540 euros\nTOTAL 8 381 euros\nPROVISION A DEDUIRE 2 500 euros\nRESTANT DÛ 5 881 euros";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DFT", Some(691.0), None),
                ("SE", Some(4000.0), None),
                ("PET", Some(150.0), None),
                ("DFP", Some(3540.0), None),
            ]
        );
    }

    #[test]
    fn a_rente_beside_the_capital_leaves_it_the_lone_sum() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation du déficit fonctionnel permanent de Mme B en l'évaluant à 205 000 euros.\nD E C I D E :\nArticle 1er : Le centre hospitalier est condamné à verser à Mme E B la somme de 302 719,10 euros, outre une rente annuelle de 9 417,14 euros payable par trimestre.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].net_victime, Some(302_719.10));
    }

    #[test]
    fn an_appeal_that_rewrites_the_judgment_pays_the_new_sum() {
        let text = "COUR ADMINISTRATIVE D'APPEL\nARRET\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation des souffrances endurées de Mme D en les évaluant à 4 000 euros.\nDECIDE :\nArticle 1er : La somme de 8 415 euros que le centre hospitalier de Nevers a été condamné à verser à Mme F D par le jugement du tribunal administratif de Dijon du 24 avril 2017 est portée à 10 075 euros sous déduction de la provision déjà versée de 2 500 euros.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].net_victime, Some(10_075.0));
    }

    #[test]
    fn the_heirs_of_the_deceased_are_the_direct_victim() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation des souffrances endurées de M. B G en les évaluant à 20 000 euros.\nD E C I D E :\nArticle 1er : Le centre hospitalier est condamné à verser aux ayants-droits de M. B G la somme de 216 273,10 euros.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].role, "directe");
        assert_eq!(v[0].net_victime, Some(216_273.10));
    }

    #[test]
    fn a_dispositif_that_prices_nothing_pays_its_lone_sum_whole() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\n12. Compte tenu de la faute d'imprudence commise par Mme A, de nature à exonérer pour moitié la commune de sa responsabilité, il sera fait une juste appréciation des souffrances endurées en les évaluant à 2 000 euros.\nD E C I D E :\nArticle 1er : La commune du Quesnoy est condamnée à verser à Mme A la somme de 8 527 euros.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].net_victime, Some(8_527.0));
    }

    #[test]
    fn a_clause_that_writes_its_labels_first_hangs_none_on_the_figure_before() {
        let text = "COUR D'APPEL\nARRET\nPAR CES MOTIFS\nCONDAMNE M. [T] à payer à M. [S] les sommes de : au titre des frais médicaux restés à charge 56 euros au titre du déficit fonctionnel temporaire 396 euros au titre des souffrances endurées 2 000 euros au titre du préjudice esthétique permanent 1 000 euros ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes[0];
        let of = |code: &str| {
            v.postes
                .iter()
                .find(|p| p.poste.as_deref() == Some(code))
                .and_then(|p| p.montant_alloue)
        };
        assert_eq!(of("PEP"), Some(1_000.0), "{:?}", v.postes);
        // Each figure keeps its own label or none: no label reaches one rank too
        // far, 396 to souffrances endurées or 2 000 to préjudice esthétique.
        let shifted = |m: f64| v.postes.iter().any(|p| p.montant_alloue == Some(m));
        assert!(!shifted(396.0) && !shifted(2_000.0), "{:?}", v.postes);
    }

    #[test]
    fn the_leaders_of_a_tabulated_line_bind_the_label_to_the_figure_behind_them() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nCONDAMNE la SA AXA à payer à Monsieur [I] [P] les sommes suivantes : au titre des dépenses de santé actuelles ………… 15,50 € au titre de l'assistance tierce personne …….……… 680 € au titre du déficit fonctionnel temporaire ………... 235 € au titre des souffrances endurées ……………….. 4.000 € au titre du préjudice d'agrément……...………… 1.000 € soit un total de 5.930,50 euros ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DSA", Some(15.5), Some(15.5)),
                ("DFT", Some(235.0), Some(235.0)),
                ("SE", Some(4_000.0), Some(4_000.0)),
                ("PA", Some(1_000.0), Some(1_000.0)),
            ]
        );
        // The same lines, each opened on a dash before its label.
        let dashed = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nCONDAMNE la MAIF à payer à Monsieur [N] [K] les sommes suivantes : \u{2013} au titre des frais divers ............... 900,00 € \u{2013} au titre du déficit fonctionnel temporaire ................... 499,20 € \u{2013} au titre des souffrances endurées ............................. 3 700,00 € \u{2013} au titre du préjudice esthétique temporaire…………………….. 500,00 € \u{2013} au titre du déficit fonctionnel permanent ................. 2 800,00 € soit un total de 8 399,20 euros ;";
        let x = extract(&scan(dashed), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("FD", Some(900.0), Some(900.0)),
                ("DFT", Some(499.2), Some(499.2)),
                ("SE", Some(3_700.0), Some(3_700.0)),
                ("PET", Some(500.0), Some(500.0)),
                ("DFP", Some(2_800.0), Some(2_800.0)),
            ]
        );
    }

    #[test]
    fn a_bullet_before_the_figure_gives_it_the_label_written_behind() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nCONDAMNE Mme [W] [S] à payer à M. [I] [N] les sommes suivantes : \u{2013} 2941,25 € de dommages et intérêts au titre de son déficit fonctionnel temporaire, \u{2013} 4120 € de dommages et intérêts au titre de son déficit fonctionnel permanent, \u{2013} 20 000 € de dommages et intérêts au titre des souffrances endurées, \u{2013} 2000 € de dommages et intérêts au titre du préjudice esthétique permanent ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DFT", Some(2_941.25), Some(2_941.25)),
                ("DFP", Some(4_120.0), Some(4_120.0)),
                ("SE", Some(20_000.0), Some(20_000.0)),
                ("PEP", Some(2_000.0), Some(2_000.0)),
            ]
        );
        let degree = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nFIXE l'indemnisation des préjudices subis par M [U] [Y] comme suit : ° 2 910.60 euros de déficit fonctionnel temporaire ° 4 100 euros d'assistance par tierce personne ° 4 000 euros de souffrances endurées ° 500 euros de préjudice esthétique temporaire ° 500 euros de préjudice esthétique permanent Soit un total de 12 010.60 euros ;";
        let x = extract(&scan(degree), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DFT", Some(2_910.6), None),
                ("SE", Some(4_000.0), None),
                ("PET", Some(500.0), None),
                ("PEP", Some(500.0), None),
            ]
        );
    }

    #[test]
    fn a_minus_sign_before_the_sum_makes_it_a_balance_and_not_an_award() {
        let text = "COUR D'APPEL\nARRET\nPAR CES MOTIFS\nFixe le préjudice de Mme [Z] au titre : - de l'assistance tierce personne temporaire à la somme de 30.901,60 euros, - de la perte de gains professionnels actuels à la somme de - 16. 919,16 euros, déduction faite des sommes versées à tort par le Trésor public, - de l'incidence professionnelle à la somme de 5 000 euros,";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![("FD", Some(30_901.6), None), ("IP", Some(5_000.0), None)]
        );
    }

    #[test]
    fn a_figure_stacked_under_another_leaves_the_label_on_the_next_line_alone() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nFIXE les préjudices subis par Mme [X] [L] de la manière suivante :\n\nAu titre du déficit fonctionnel temporaire total => 12js x 25 euros\n300 euros\nAu titre du déficit fonctionnel temporaire partiel\n- 15 jours de déficit de 50% = 25 euros x 0,50 x 15 js\n- 30 jours de déficit de 25% = 25 euros x 0,25 x 30 js\n\n187,50 euros\n187,50 euros\nau titre des souffrances endurées évaluées à 2/7\n- physiques\n- psychiques\n\n2.500 euros\n1.500 euros\nau titre du préjudice d'angoisse\n500 euros\nau titre du préjudice d'agrément\n500 euros\n\n9.675 euros\n\nCONDAMNE Madame [G] [R] à régler à Mme [X] [L]\n- la somme de 9.675 euros en réparation de ses entiers préjudices,";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        let se: Vec<_> = v.iter().flat_map(postes).filter(|p| p.0 == "SE").collect();
        assert!(se.is_empty(), "{v:?}");
    }

    #[test]
    fn a_caisse_named_in_lowercase_opens_a_payee_whose_sums_leave_the_record() {
        let text = "COUR D'APPEL\nARRET\nPAR CES MOTIFS\nCondamne in solidum M. Mohamed Z..., l'association Racing club Carpentras XIII et la société Covea Risk à payer : à M. Arnaud X... la somme de 665.208 € en réparation de son préjudice patrimonial et extra patrimonial, à M. Vincent X... la somme de 7.500 € en réparation de son préjudice moral, à la caisse primaire d'assurance-maladie du Tarn et de l'Aveyron la somme de 1.143.647,11 € au titre des dépenses de santé et frais médicaux futurs et la somme de 980 € au titre de l'indemnité forfaitaire,";
        let x = extract(&scan(text), Mode::Sure);
        for m in [1_143_647.11, 980.0] {
            assert!(
                x.obligations
                    .iter()
                    .any(|o| o.montant == m && o.nature == Nature::HorsRecord("tiers_payeur")),
                "{:?}",
                x.obligations
            );
        }
        let text = "COUR D'APPEL\nARRET\nPAR CES MOTIFS\nCondamne in solidum M. Mohamed Z... et la société Covea Risk à payer aux consorts X... d'une part la somme de 2000 €, à la caisse primaire d'assurance-maladie du Tarn et de l'Aveyron d'autre part la somme de 500 € sur le fondement de l'article 700 du code de procédure civile.";
        let x = extract(&scan(text), Mode::Sure);
        for m in [2_000.0, 500.0] {
            assert!(
                x.obligations
                    .iter()
                    .any(|o| o.montant == m && o.nature == Nature::HorsRecord("article_700")),
                "{:?}",
                x.obligations
            );
        }
    }

    #[test]
    fn a_married_name_says_whose_wife_she_is_and_not_what_she_is_to_the_victim() {
        let text = "COUR D'APPEL\nARRET\nPAR CES MOTIFS\nCondamne in solidum M. Mohamed Z... et la société Covea Risk à payer :\n\nà M. Arnaud X...\n\nla somme de 665.208 € en réparation de son préjudice patrimonial et extra patrimonial,\n\nune rente viagère trimestrielle de 4.950 € au titre de l'incidence professionnelle qui sera réévaluée le 1er janvier de chaque année,\n\nà M. Michel X... la somme de 15.000 € en réparation de son préjudice moral,\n\nà Madame Michèle B... épouse X... la somme de 15.000 € en réparation de son préjudice moral,\n\nà M. Vincent X... la somme de 7.500 € en réparation de son préjudice moral,";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 4, "{v:?}");
        assert!(v.iter().all(|w| w.lien.is_none()), "{v:?}");
    }

    #[test]
    fn the_registry_typo_on_a_rente_still_keeps_its_arrears_out_of_the_net() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\nQuant aux souffrances endurées :\n12. Il sera fait une juste appréciation de ce chef de préjudice en lui allouant la somme de 10 000 euros.\nD E C I D E :\nArticle 1er : Les HUS sont condamnés à verser à Mme D A la somme de 90 742,68 euros, déduction faite de la provision de 30 000 euros allouée par les assureurs des HUS.\nArticle 2 : Les HUS sont condamnés à verser à Mme D A, à compter de la présente décision, une rentre annuelle d'un montant de 6 645,56 euros euros (six mille six cent quarante-cinq euros et cinquante-six centimes). Le montant de cette rente, qui est payable à terme échu, sera revalorisée.\nArticle 4 : Les HUS verseront à Mme D A la somme de 88,80 euros au titre de l'article R. 761-1 du code de justice administrative.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].net_victime, None, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("SE", Some(10_000.0), None)]);
    }

    #[test]
    fn a_sum_spelt_out_in_brackets_still_hangs_its_label_on_the_figure_before() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nFIXE l'indemnisation des préjudices de M. [T] [K] comme suit : la somme de 6.000€ (six mille euros) au titre des souffrances endurées avant consolidation,la somme de 2.000€ (deux mille euros) au titre du préjudice esthétique temporaire, la somme de 500€ (cinq cents euros) au titre du préjudice d'agrément,la somme de 1.641,25€ (mille six cent quarante et un euros et vingt-cinq centimes) au titre du déficit fonctionnel temporaire,la somme de 7.840€ (sept mille huit cent quarante euros) au titre du déficit fonctionnel permanent, la somme de 64,86€ (soixante-quatre euros et quatre-vingt-six centimes) au titre des frais de déplacement.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("SE", Some(6_000.0), None),
                ("PET", Some(2_000.0), None),
                ("PA", Some(500.0), None),
                ("DFT", Some(1_641.25), None),
                ("DFP", Some(7_840.0), None),
            ]
        );
    }

    #[test]
    fn a_label_hung_by_titre_alone_stays_on_the_figure_before_it() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nPAR CES MOTIFS\nCondamne la SA AXA FRANCE IARD à payer à madame [V] [G] épouse [P] les sommes suivantes :\n112€ au titre du déficit fonctionnel temporaire,2 000 € titre des souffrances endurées,500 € titre du préjudice esthétique temporaire,";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DFT", Some(112.0), Some(112.0)),
                ("SE", Some(2_000.0), Some(2_000.0)),
                ("PET", Some(500.0), Some(500.0)),
            ]
        );
    }

    #[test]
    fn the_chiefs_listed_under_an_open_exception_recite_the_judgment() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nConfirme le jugement déféré sauf :\n* en ce qu'il a fixé et évalué les préjudices suivants causés à Mme [O] [I] :\n- frais divers : 2 405,00 euros\n- souffrances endurées : 30 000 euros\n* en ce qu'il a condamné la Macif à payer la somme 161 930,45 euros pour l'indemnisation des lésions initiales à Mme [O] [I] en réparation des préjudices déduction à faire des provisions versées par la Macif ;\nStatuant à nouveau sur les chefs de jugement infirmés et y ajoutant,\nFixe et évalue les préjudices causés à Mme [I] comme suit :\n- frais divers : 2 500,00 euros\n- souffrances endurées : 50 000 euros\nCondamne la Macif à payer à Mme [O] [I] la somme de 220 186,94 euros en réparation de ses préjudices ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![("FD", Some(2_500.0), None), ("SE", Some(50_000.0), None)]
        );
    }

    #[test]
    fn a_figure_without_a_cue_opening_the_payee_items_leaves_the_next_label_to_its_own_figure() {
        let text = "COUR D'APPEL\nARRET\nPAR CES MOTIFS\nCondamne in solidum le docteur [T] et la clinique du [13] à payer à Mme [K] les sommes suivantes :\no assistance par tierce personne : 2 071,90 euros\no déficit fonctionnel temporaire : 915 euros\no souffrances endurées : 6 000 euros\no préjudice esthétique temporaire : 500 euros";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DFT", Some(915.0), Some(915.0)),
                ("SE", Some(6_000.0), Some(6_000.0)),
                ("PET", Some(500.0), Some(500.0)),
            ]
        );
    }

    #[test]
    fn a_pseudonym_written_on_its_dots_is_a_name() {
        let text = "COUR ADMINISTRATIVE D'APPEL\nARRET\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation des souffrances endurées en les évaluant à 4 000 euros.\nDECIDE :\nArticle 1er : La somme de 8 415 euros que le centre hospitalier de Nevers a été condamné à verser à Mme F...D...par le jugement du tribunal administratif de Dijon du 24 avril 2017 est portée à 10 075 euros.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].net_victime, Some(10_075.0));
    }

    #[test]
    fn two_spellings_of_a_pseudonym_are_one_payee() {
        let text = "COUR ADMINISTRATIVE D'APPEL\nARRET\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation des souffrances endurées en les évaluant à 4 000 euros.\nDÉCIDE :\nArticle 1er : L'indemnité de 707 986 euros, qui a été mise à la charge du centre hospitalier au profit de M. C... par le jugement du 26 juin 2018, est portée à 715 310,22 euros.\nArticle 2 : Le centre hospitalier est condamné à verser à M. C... à compter du présent arrêt une rente trimestrielle de 6 586,34 euros.";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].net_victime, Some(715_310.22));
    }

    #[test]
    fn two_payees_of_a_lone_sum_name_no_victim_at_all() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation des préjudices en cause.\nD E C I D E :\nArticle 1er : Le centre hospitalier est condamné à verser la somme de 55 162,90 euros à Mme E B et la somme de 1 666 euros à M. H A.";
        let x = extract(&scan(text), Mode::Sure);
        assert!(x.quantum.victimes.is_empty(), "{:?}", x.quantum.victimes);
        assert_eq!(x.obligations.len(), 2);
    }

    #[test]
    fn a_text_that_names_an_indirect_victim_gives_the_lone_sum_no_role() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nMme [U] [B], victime indirecte, demande réparation.\nPAR CES MOTIFS\nLa cour,\nCondamne la compagnie Groupama à payer à Mme [U] [B] la somme globale de 20 000 euros en réparation de ses préjudices ;";
        let x = extract(&scan(text), Mode::Sure);
        assert!(x.quantum.victimes.is_empty(), "{:?}", x.quantum.victimes);
    }

    #[test]
    fn one_article_in_francs_does_not_price_an_arret_in_euros() {
        let text = "COUR ADMINISTRATIVE D'APPEL\nARRET\nConsidérant ce qui suit :\nSur les préjudices :\n16. Il sera fait une juste appréciation des souffrances endurées en allouant la somme de 13 000 euros.\nD E C I D E :\nArticle 1er : Le centre hospitalier de la Polynésie française est condamné à verser à Mme D la somme de 14 000 euros.\nArticle 2 : Les frais d'expertise liquidés et taxés à la somme de 304 500 F CFP sont mis à la charge du centre hospitalier.";
        let x = extract(&scan(text), Mode::Sure);
        assert_eq!(x.quantum.devise.as_deref(), Some("EUR"));
    }

    #[test]
    fn a_judgment_of_papeete_names_the_currency_it_prices_in() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nSur les préjudices :\n12. Il sera fait une juste appréciation des souffrances endurées en allouant la somme de 3 736 000 F CFP.\nD E C I D E :\nArticle 1er : L'Etat est condamné à verser à Mme B une somme de 7 316 842 F CFP.";
        let x = extract(&scan(text), Mode::Sure);
        assert_eq!(x.quantum.devise.as_deref(), Some("XPF"));
        assert_eq!(x.quantum.victimes[0].net_victime, Some(7_316_842.0));
        assert_eq!(
            extract(&scan(TA), Mode::Sure).quantum.devise.as_deref(),
            Some("EUR")
        );
    }

    #[test]
    fn a_payment_with_its_postes_makes_a_ricochet_victim() {
        let x = extract(&scan(TJ), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].role, "indirecte");
        assert_eq!(v[0].net_victime, Some(38017.83));
        assert_eq!(
            postes(&v[0]),
            vec![
                ("PR", Some(13017.83), Some(13017.83)),
                ("PAF", Some(25000.0), Some(25000.0)),
            ]
        );
        assert!(x
            .obligations
            .iter()
            .any(|o| o.montant == 15000.0 && o.nature == Nature::HorsRecord("article_700")));
    }

    #[test]
    fn a_fixed_list_links_by_adjacency_in_both_directions() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur le préjudice.\nPAR CES MOTIFS\nLa cour,\nFixe à la somme globale de 3 025 euros les préjudices extra-patrimoniaux de M. [K], à savoir, 1 025 euros au titre du déficit fonctionnel temporaire et 2 000 euros au titre des souffrances endurées ;\nFIXE le préjudice corporel de M. [K] comme suit : déficit fonctionnel permanent : 27 600 euros ; préjudice d'agrément : 0 ; Total : 27 600 euros ;\nCONDAMNE la société AXA à payer à M. [K] la somme totale de 47 600 euros en réparation de son préjudice corporel ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].role, "directe");
        assert_eq!(v[0].net_victime, Some(47600.0));
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DFT", Some(1025.0), None),
                ("SE", Some(2000.0), None),
                ("DFP", Some(27600.0), None),
                ("PA", Some(0.0), None),
            ]
        );
        let totals: Vec<f64> = x
            .obligations
            .iter()
            .filter(|o| matches!(o.nature, Nature::Total { .. }))
            .map(|o| o.montant)
            .collect();
        assert_eq!(totals, vec![27600.0]);
        assert!(x
            .obligations
            .iter()
            .any(|o| o.montant == 3025.0 && o.nature == Nature::HorsRecord("intermediaire")));
    }

    #[test]
    fn recitals_organisms_and_reductions_write_no_field() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nLa faute de la victime réduit son droit à indemnisation à hauteur de 50 %.\nPAR CES MOTIFS\nLa cour,\nConfirme le jugement sauf en ce qu'il a fixé à la somme de 4 356,75 euros la perte de gains professionnels actuels de M. [D] ;\nStatuant à nouveau,\nFixe le préjudice de M. [D] comme suit : perte de gains professionnels actuels : 6 331,77 euros ;\nCondamne la MAIF à payer à la CPAM la somme de 1 200 euros au titre des dépenses de santé actuelles ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("PGPA", None, None)]);
        let hors: Vec<(f64, &str)> = x
            .obligations
            .iter()
            .filter_map(|o| match o.nature {
                Nature::HorsRecord(c) => Some((o.montant, c)),
                _ => None,
            })
            .collect();
        assert_eq!(
            hors,
            vec![(4356.75, "rappel_du_jugement"), (1200.0, "tiers_payeur")]
        );
    }

    #[test]
    fn an_allocation_verb_names_its_payee_and_prices_the_poste() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nAlloue à Mme [Z] [S] les sommes suivantes :\n- 924,20 euros au titre des dépenses de santé actuelles,\n- 40 000 euros au titre des souffrances endurées ;";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DSA", Some(924.20), Some(924.20)),
                ("SE", Some(40000.0), Some(40000.0)),
            ]
        );
    }

    #[test]
    fn a_payer_named_anywhere_closes_the_patrimonial_poste_of_the_sure_mode() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices, la caisse primaire d'assurance maladie de Paris a pris en charge les dépenses de santé.\nPAR CES MOTIFS\nLa cour,\nAlloue à Mme [Z] [S] les sommes suivantes :\n- 924,20 euros au titre des dépenses de santé actuelles,\n- 40 000 euros au titre des souffrances endurées ;";
        let x = extract(&scan(text), Mode::Sure);
        assert_eq!(
            postes(&x.quantum.victimes[0]),
            vec![
                ("DSA", None, Some(924.20)),
                ("SE", Some(40000.0), Some(40000.0)),
            ]
        );
    }

    #[test]
    fn a_list_priced_after_the_reduction_writes_the_share_not_the_poste() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nFixe les préjudices de M. [O] [E], avant application de la limitation du droit à indemnisation :\n- 6 000 euros au titre des souffrances endurées ;\nFixe les préjudices de M. [O] [E], après application de la limitation du droit à indemnisation :\n- 4 200 euros au titre des souffrances endurées ;";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("SE", Some(6000.0), Some(4200.0))]);
    }

    #[test]
    fn heirs_of_a_named_deceased_open_the_direct_victim() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nCondamne la société AXA à payer à Mme [G] [O] et M. [I] [N], en leur qualité d'ayants droit de [T] [N], la somme de 20 000 euros au titre du déficit fonctionnel permanent ;";
        let sure = extract(&scan(text), Mode::Sure);
        assert_eq!(sure.quantum.victimes.len(), 1);
        assert_eq!(sure.quantum.victimes[0].role, "directe");
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].role, "directe");
        assert_eq!(postes(&v[0]), vec![("DFP", Some(20000.0), Some(20000.0))]);
    }

    #[test]
    fn a_reversal_keeps_the_chiefs_its_exception_names() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nInfirme le jugement déféré sauf en ce qu'il a :\n- condamné la compagnie AXA à verser à Mme [P] [T] 10 000 euros au titre de son préjudice d'affection,\n- débouté M. [C] de sa demande au titre du préjudice d'agrément ;\nStatuant à nouveau,\nCondamne la compagnie AXA à payer à M. [C] la somme de 5 000 euros au titre des souffrances endurées ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("PAF", Some(10000.0), Some(10000.0))]);
        assert_eq!(postes(&v[1]), vec![("SE", Some(5000.0), Some(5000.0))]);
    }

    #[test]
    fn statuant_a_nouveau_closes_the_recital_of_the_reversed_judgment() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nInfirme le jugement en ce qu'il a dit que les préjudices de M. [O] [G] doivent être indemnisés de la façon suivante : - 60 000 euros au titre des souffrances endurées ;\nConfirme le surplus ;\nStatuant à nouveau sur les chefs de la décision infirmés et y ajoutant,\nDit que les préjudices de M. [O] [G] doivent être indemnisés de la façon suivante : - 50 000 euros au titre des souffrances endurées ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("SE", Some(50000.0), None)]);
    }

    #[test]
    fn the_apposition_names_the_kinship_of_the_payee_and_not_of_her_representative() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nCondamne la MAIF à payer à Madame [L] [A], veuve de Monsieur [E] [A], la somme de 25 000 euros au titre de son préjudice d'affection ;\nCondamne la MAIF à payer à [E] [W], représentée par Madame [Y] [K] veuve [W], la somme de 10 000 euros au titre de son préjudice d'affection ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(v[0].lien, Some(Lien::Conjoint));
        assert_eq!(v[1].lien, None);
    }

    #[test]
    fn each_payee_takes_the_sum_written_once_for_all() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nCondamne la MAIF à payer à Mme [B] [A] et M. [T] [A], chacun, la somme de 1 666,67 euros en réparation de leur préjudice d'affection ;\nCondamne la MAIF à payer à Mme [G] [F] et à Mme [M] [F] la somme de 3 000 euros chacune en réparation de leur préjudice d'affection ;\nCondamne la MAIF à payer à M. [K] [A] et à Mme [D] [A] la somme de 4 000 euros en réparation de leur préjudice d'affection ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 4, "{v:?}");
        for (k, amount) in [(0, 1666.67), (1, 1666.67), (2, 3000.0), (3, 3000.0)] {
            assert_eq!(
                postes(&v[k]),
                vec![("PAF", Some(amount), Some(amount))],
                "{k}"
            );
        }
    }

    #[test]
    fn the_motifs_price_a_poste_the_dispositif_only_confirms() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS DE LA DECISION\nSur le déficit fonctionnel permanent, il sera alloué à M. [D] la somme de 12 000 euros.\nPAR CES MOTIFS\nLa cour,\nConfirme le jugement en toutes ses dispositions ;\nCondamne la société AXA aux dépens.";
        assert!(extract(&scan(text), Mode::Sure).quantum.victimes.is_empty());
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("DFP", Some(12000.0), None)]);
    }
    #[test]
    fn a_moral_prejudice_paid_beside_the_direct_victim_is_the_payee_s_affection() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nFixe comme suit les préjudices subis par M. [O] [Y] :\n- le déficit fonctionnel permanent : 160 000 euros\n- les souffrances endurées : 35 000 euros\nCondamne la société Groupama à payer à Mme [U] [Y] et à M. [Z] [Y] la somme de 10 000 euros à chacun en réparation de leur préjudice moral et d'accompagnement,";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[2].role, "directe");
        for k in [0, 1] {
            assert_eq!(v[k].role, "indirecte");
            assert_eq!(postes(&v[k]), vec![("PAF", Some(10000.0), Some(10000.0))]);
        }
    }
    #[test]
    fn a_label_written_behind_its_amount_does_not_price_the_amount_after_it() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nFixe les préjudices subis par M. [D] [A] aux sommes suivantes :\n- 11 300 euros au titre du déficit fonctionnel permanent,\n- 5 500 euros au titre des tracas administratifs,\n- 4 600 euros au titre du préjudice d'agrément,";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![("DFP", Some(11300.0), None), ("PA", Some(4600.0), None)]
        );
    }
    #[test]
    fn a_compound_kinship_is_not_outvoted_by_the_word_it_contains() {
        assert_eq!(
            kinship_of("madame [m] [o], petite-fille de monsieur [e] [a]"),
            Some(Lien::PetitEnfant)
        );
        assert_eq!(
            kinship_of("monsieur [y] [u], petit-fils de monsieur [e] [a]"),
            Some(Lien::PetitEnfant)
        );
        assert_eq!(
            kinship_of("madame [w] [a], fille de monsieur [e] [a]"),
            Some(Lien::Enfant)
        );
        assert_eq!(kinship_of("madame [l] [a], veuve et fille"), None);
    }

    #[test]
    fn a_fixing_clause_shares_one_head_of_damage_between_named_relatives() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLe tribunal,\nFIXE l'indemnisation des préjudices moraux des ayants droit de Monsieur [E] [A] comme suit :\n32.600 € au bénéfice de Madame [L] [A], veuve de Monsieur [E] [A];\n8.700 € au bénéfice de Madame [W] [O], fille de Monsieur [E] [A] ;\n3.300 € au bénéfice de Madame [M] [O], petite-fille de Monsieur [E] [A] ;\nSoit un total de 44.600 € ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 3, "{v:?}");
        for (k, lien, amount) in [
            (0, Lien::Conjoint, 32600.0),
            (1, Lien::Enfant, 8700.0),
            (2, Lien::PetitEnfant, 3300.0),
        ] {
            assert_eq!(v[k].role, "indirecte");
            assert_eq!(v[k].lien, Some(lien));
            assert_eq!(postes(&v[k]), vec![("PAF", Some(amount), None)]);
        }
    }
    #[test]
    fn a_chief_the_appeal_excepts_is_replaced_and_not_confirmed() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLa cour,\nConfirme le jugement sauf en ce qu'il a fixé à la somme de 4 356,75 euros la perte de gains professionnels actuels de M. [D] [A] ;\nStatuant à nouveau,\nFixe le préjudice de M. [D] [A] comme suit : perte de gains professionnels actuels : 6 331,77 euros ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("PGPA", Some(6331.77), None)]);
    }

    #[test]
    fn a_kinship_word_the_payee_owns_names_the_other_party() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLe tribunal,\nFIXE le préjudice corporel de [O] [D] à la somme de 2.417,78 € décomposée comme suit :\n- DSA : 164,03 €\n- ATP : 375 €\n- DFT : 303,75 €\n- SE : 1.500 €\n- PA : 75 €\nCONDAMNE monsieur [Y] [T] [E] à payer à madame [U] [I] épouse [D], responsable légale de son enfant mineur, la somme de 2.417,78 €, en réparation du préjudice corporel de son fils mineur [O] [D] ;\nCONDAMNE monsieur [Y] [T] [E] à payer à madame [U] [I] épouse [D], la somme de 1.000 € en réparation de son préjudice d'affection, consécutif à l'accident survenu à ses enfants mineurs ;";
        let x = extract(&scan(text), Mode::Sure);
        let mother = x
            .quantum
            .victimes
            .iter()
            .find(|v| v.postes.iter().any(|p| p.poste.as_deref() == Some("PAF")))
            .expect("the mother");
        assert_eq!(mother.lien, Some(Lien::Parent), "{:?}", x.quantum.victimes);
        assert_eq!(
            kinship_of("madame [u] [i], la somme de 1.000 € en reparation de son prejudice d'affection, consecutif a l'accident survenu a ses enfants mineurs"),
            Some(Lien::Parent)
        );
        assert_eq!(
            kinship_of("monsieur [k], en reparation du prejudice resultant du deces de sa mere"),
            Some(Lien::Enfant)
        );
    }

    #[test]
    fn the_fee_of_the_legal_aid_lawyer_is_an_article_700() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLe tribunal,\nFixe le montant des indemnités revenant à Monsieur [K] [J] aux sommes suivantes:\n- souffrances endurées : 4 000,00 €\n- déficit fonctionnel temporaire : 1 344,00 €\nCondamne Monsieur [R] [A] à payer à Maître X..., conseil de Monsieur [K] [J], la somme de 1 700 € en application de l'article 37 de la loi du 10 juillet 1991 ;";
        let x = extract(&scan(text), Mode::Sure);
        assert!(
            x.obligations
                .iter()
                .any(|o| o.montant == 1700.0 && o.nature == Nature::HorsRecord("article_700")),
            "{:?}",
            x.obligations
        );
    }

    #[test]
    fn an_instalment_and_a_provision_ad_litem_leave_the_record() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS\nSur les préjudices.\nPAR CES MOTIFS\nLe tribunal,\nCONDAMNE Monsieur [R] [P] à payer à Monsieur [H] [Q] la somme de 2.229,75 euros au titre de son préjudice corporel se décomposant comme suit :\n- dépenses de santé futures : 917,25 euros\n- déficit fonctionnel temporaire : 12,5 euros\n- souffrances endurées : 800 euros\n- préjudice esthétique temporaire : 500 euros\nDIT que cette somme portera intérêt au taux légal à compter de la présente décision ;\nAUTORISE Monsieur [R] [P] à s'acquitter des sommes susvisées en 24 mensualités de 92,91 €, le 5 de chaque mois ;\nRAPPELLE que Monsieur [R] [P] a versé des sommes provisionnelles pour un montant total de MONTANTPROVISION euros, outre une provision ad litem à hauteur de 1.000€ ;\nCONDAMNE Monsieur [R] [P] à payer à Monsieur [H] [Q] la somme de 1.000 euros en application des dispositions de l'article 700 du code de procédure civile ;";
        let x = extract(&scan(text), Mode::Sure);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DSF", Some(917.25), Some(917.25)),
                ("DFT", Some(12.5), Some(12.5)),
                ("SE", Some(800.0), Some(800.0)),
                ("PET", Some(500.0), Some(500.0))
            ]
        );
        assert_eq!(v[0].net_victime, Some(2229.75), "{v:?}");
        assert!(
            x.obligations
                .iter()
                .any(|o| o.montant == 92.91 && matches!(o.nature, Nature::HorsRecord(_))),
            "{:?}",
            x.obligations
        );
        assert!(
            x.obligations
                .iter()
                .any(|o| o.montant == 1000.0 && o.nature == Nature::HorsRecord("provision")),
            "{:?}",
            x.obligations
        );
    }

    #[test]
    fn a_poste_refused_in_the_sentence_that_names_it_opens_no_section() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS DE LA DÉCISION\nSur les frais d'assistance par une tierce personne :\n\nDans le cas où la victime a besoin, du fait de son incapacité temporaire, d'être assistée avant la consolidation par une tierce personne, elle a le droit à l'indemnisation du coût du recours à cette tierce personne.\n\nL'assistance par une tierce personne après consolidation ne peut en revanche faire l'objet d'une indemnisation complémentaire dès lors que ce préjudice est couvert par les dispositions de l'article L. 434-2 du code de la sécurité sociale.\n\nL'expert a retenu la nécessité d'une assistance à hauteur d'une heure par jour qui n'est pas discutée.\n\nCe préjudice sera indemnisé à hauteur de 1 200 €.\n\nSur le déficit fonctionnel permanent :\n\nCe préjudice sera indemnisé à hauteur de 4 200 €.\n\nPAR CES MOTIFS\nLe tribunal,\nFixe le montant des indemnités revenant à Monsieur [K] [J] aux sommes suivantes:\n- déficit fonctionnel permanent : 4 200,00 €\n- tierce personne : 1 200,00 €\n";
        for mode in [Mode::Sure, Mode::Extended] {
            let x = extract(&scan(text), mode);
            let v = &x.quantum.victimes;
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(
                v[0].postes
                    .iter()
                    .all(|p| p.poste.as_deref() != Some("ATP")),
                "{v:?}"
            );
        }
    }

    #[test]
    fn a_poste_named_in_the_body_of_a_section_prices_nothing_past_its_paragraph() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS DE LA DÉCISION\nSur le déficit fonctionnel temporaire :\n\nIl s'agit de l'invalidité subie par la victime dans sa sphère personnelle jusqu'à sa consolidation. Cette invalidité correspond aux périodes d'hospitalisation de la victime, mais aussi à la perte de qualité de vie, la privation temporaire des activités privées ou d'agrément auxquelles la victime se livrait habituellement et le préjudice sexuel avant consolidation.\n\nCe préjudice sera indemnisé à hauteur de la somme demandée, soit 1 344 €.\n\nSur les souffrances endurées :\n\nCe poste sera indemnisé à hauteur de 4 000 euros.\n\nPAR CES MOTIFS\nLe tribunal,\nFixe le montant des indemnités revenant à Monsieur [K] [J] aux sommes suivantes:\n- souffrances endurées : 4 000,00 €\n- déficit fonctionnel temporaire : 1 344,00 €\n";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(
            v[0].postes.iter().all(|p| p.poste.as_deref() != Some("PS")),
            "{v:?}"
        );
    }

    #[test]
    fn a_heading_the_vocabulary_does_not_know_closes_the_section_above_it() {
        let text = "TRIBUNAL ADMINISTRATIF\nJUGEMENT\nConsidérant ce qui suit :\nS'agissant du préjudice sexuel :\n20. Il ne résulte pas de l'instruction que Mme B souffre d'un préjudice sexuel permanent en lien avec celui-ci. Par conséquent, sa demande d'indemnisation à ce titre doit être rejetée.\nS'agissant du préjudice particulier tiré de l'altération du lien avec son nouveau-né :\n21. Mme B soutient qu'elle a subi un préjudice particulier lié à l'altération des liens avec son nouveau-né, qu'elle évalue à la somme de 3 000 euros. Toutefois, elle ne fait pas état d'un chef de préjudice distinct. Aussi, cette demande doit être rejetée.\n22. Il résulte de tout ce qui précède que l'AP-HP doit être condamnée à verser la somme de 636 euros à Mme B.\nD E C I D E :\nArticle 1er : L'AP-HP est condamnée à verser à Mme B la somme de 636 euros.\nArticle 2 : L'AP-HP versera à Mme B la somme de 1 500 euros au titre de l'article L. 761-1 du code de justice administrative.";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert!(
            v.iter()
                .all(|w| w.postes.iter().all(|p| p.poste.as_deref() != Some("PS"))),
            "{v:?}"
        );
    }

    #[test]
    fn the_name_of_a_section_is_read_past_the_window_that_finds_it() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS DE LA DÉCISION\n2. Sur l'évaluation du préjudice\n\nAu vu de l'ensemble des éléments versés aux débats, le préjudice subi par [R] [C], né le 4 mai 1990, sera évalué comme suit.\n\nIncidence professionnelle\n\nCe poste sera indemnisé à hauteur de 150 000 euros.\n\nPAR CES MOTIFS\nLe tribunal,\nCondamne l'ONIAM à payer à [R] [C] la somme de 150 000 euros au titre de l'incidence professionnelle\n";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(
            v[0].postes
                .iter()
                .any(|p| p.poste.as_deref() == Some("IP") && p.montant_alloue == Some(150000.0)),
            "{v:?}"
        );
    }

    #[test]
    fn a_poste_priced_by_parts_takes_neither_part() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS DE LA DÉCISION\n2.3.1) Déficit fonctionnel temporaire\n\nIl sera retenu une base journalière de 30 €.\n\n* Déficit fonctionnel temporaire total\n\nIl couvre la période du 23 décembre 2015 au 29 mars 2016, soit 134 jours. L'indemnité sera fixée à 4 020 € (134 j x 30 €).\n\n* Déficit fonctionnel temporaire partiel\n\nIl couvre 963 jours pour un taux de 75 %. L'indemnité sera fixée à 21 668 € (963 j x 30 € x 75 %).\n\nLes troubles dans les conditions d'existence subis jusqu'à la consolidation justifient l'octroi d'une somme totale de 25 688 €.\n\n2.3.2) Souffrances endurées\n\nCe poste sera indemnisé à hauteur de 4 000 €.\n\nPAR CES MOTIFS\nLe tribunal,\nCondamne la société [X] à payer à Monsieur [K] [J] la somme de 29 688 € en réparation de son préjudice corporel\n";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert!(
            v.iter()
                .all(|w| w.postes.iter().all(|p| p.poste.as_deref() != Some("DFT"))),
            "{v:?}"
        );
    }

    #[test]
    fn a_verb_whose_object_is_plural_prices_the_whole_and_no_poste() {
        let text = "COUR D'APPEL\nARRÊT\nMOTIFS DE LA DÉCISION\nSur le préjudice d'agrément\n\nL'expert évoque un arrêt définitif de l'équitation. La cour considère qu'il y a lieu d'évaluer ce préjudice en tenant compte de l'impossibilité de faire de l'équitation, évaluée à 7 500 euros.\n\nIl ressort de ce qui précède qu'il y a lieu d'évaluer à la somme de 192 500 euros l'indemnisation à laquelle Mme [S] peut prétendre dans le cadre des préjudices examinés en appel.\n\nPAR CES MOTIFS\nLa cour,\nLiquide le préjudice corporel subi par Mme [Y] [S] à la somme de 192 500 euros,\nCondamne la S.A. [P] à payer à Mme [Y] [S] la somme de 192 500 euros au titre de son préjudice corporel\n";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        let pa: Vec<Option<f64>> = v
            .iter()
            .flat_map(|w| &w.postes)
            .filter(|p| p.poste.as_deref() == Some("PA"))
            .map(|p| p.montant_alloue)
            .collect();
        assert_eq!(pa, vec![Some(7500.0)], "{v:?}");
    }

    #[test]
    fn a_verb_in_the_parties_claims_awards_nothing() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nEXPOSÉ DU LITIGE\nLa société [K] demande au tribunal de : ramener à de plus justes proportions la somme à allouer à Mme [B] au titre de l'incidence professionnelle ; fixer à la somme de 222 685,76 euros le montant global de l'indemnité au titre de l'assistance par tierce personne.\nMOTIFS DE LA DÉCISION\nSur l'incidence professionnelle\n\nIl y a lieu de réduire à la somme de 20 000 euros l'indemnisation de ce poste de préjudice.\n\nPAR CES MOTIFS\nLe tribunal,\nCondamne la société [K] à payer à Mme [B] la somme de 20 000 euros au titre de l'incidence professionnelle\n";
        let x = extract(&scan(text), Mode::Extended);
        let v = &x.quantum.victimes;
        assert!(
            v.iter()
                .flat_map(|w| &w.postes)
                .all(|p| p.montant_alloue != Some(222685.76)),
            "{v:?}"
        );
    }

    /// The nomenclature files temporary aid under frais divers, and a court that
    /// abbreviates it « ATP » says nothing of the phase.
    #[test]
    fn a_bare_atp_abbreviation_opens_no_permanent_assistance_poste() {
        let text = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS DE LA DÉCISION\n\
                    L'aide apportée sera évaluée à 18 euros x 1,5 h x 15 jours par enfant.\n\
                    PAR CES MOTIFS\nLe tribunal,\n\
                    FIXE le préjudice corporel de [O] [D] à la somme de 2 417,78 euros décomposée comme suit :\n\
                    - DSA : 164,03 €\n- ATP : 375 €\n- SE : 1 500 €\n";
        let x = extract(&scan(text), Mode::Sure);
        let codes: Vec<&str> = x.quantum.victimes[0]
            .postes
            .iter()
            .map(|p| p.poste.as_deref().unwrap())
            .collect();
        assert!(!codes.contains(&"ATP"), "{codes:?}");
    }
}
