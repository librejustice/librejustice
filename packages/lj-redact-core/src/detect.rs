//! Rule-based suggestions: what a French legal document usually hides.
//!
//! The detector proposes strings; the search and the removal are the same ones a
//! typed string goes through, so a suggestion carries no privilege and no
//! separate path. Hand-rolled rather than regex-driven: the whole module has to
//! reach the browser, where a regex engine costs more than the document.

use crate::fold::fold_char;
use crate::index::Anchor;

/// What a suggestion is, which is what the interface groups them by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A name carried by a civility, or a surname written in capitals.
    Person,
    /// A postal address, from its house number to its city.
    Address,
    Email,
    Phone,
    /// Social security, company registration, bank account.
    Identifier,
    /// A docket or case number.
    Docket,
    /// A date of birth, which identifies as surely as a name.
    BirthDate,
    /// The town named with a birth, which the user weighs: it is often also
    /// the seat of a court.
    BirthPlace,
}

impl Kind {
    const ALL: [Kind; 8] = [
        Kind::Person,
        Kind::Address,
        Kind::Email,
        Kind::Phone,
        Kind::Identifier,
        Kind::Docket,
        Kind::BirthDate,
        Kind::BirthPlace,
    ];

    /// The name the page files the family under.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Person => "person",
            Kind::Address => "address",
            Kind::Email => "email",
            Kind::Phone => "phone",
            Kind::Identifier => "identifier",
            Kind::Docket => "docket",
            Kind::BirthDate => "birth-date",
            Kind::BirthPlace => "birth-place",
        }
    }

    /// The family [`Self::name`] gives `name`.
    pub fn named(name: &str) -> Option<Kind> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// One proposed string, with how many times the document carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub text: String,
    pub kind: Kind,
    pub count: usize,
}

pub(crate) const CIVILITIES: [&str; 22] = [
    "m",
    "mm",
    "mme",
    "mmes",
    "mlle",
    "mlles",
    "monsieur",
    "messieurs",
    "madame",
    "mesdames",
    "mademoiselle",
    "me",
    "maitre",
    "maitres",
    "mtre",
    "dr",
    "docteur",
    "pr",
    "professeur",
    "mr",
    "mrs",
    "ms",
];

/// Civilities that open a list: `MM. Durel, Fabre et Morin, conseillers`.
const PLURALS: [&str; 6] = ["mm", "mmes", "mlles", "messieurs", "mesdames", "maitres"];

/// Law firms, named after their partners: `SCP Lemoine, Barre et Cosson`.
const FIRMS: [&str; 2] = ["scp", "selarl"];

/// Words after which a decision names a relative by first name alone.
const KIN: [&str; 30] = [
    "fils",
    "fille",
    "filles",
    "enfant",
    "enfants",
    "petits-enfants",
    "petit-fils",
    "petite-fille",
    "petites-filles",
    "epouse",
    "epoux",
    "frere",
    "freres",
    "soeur",
    "soeurs",
    "mere",
    "pere",
    "compagne",
    "compagnon",
    "concubin",
    "concubine",
    "conjoint",
    "conjointe",
    "jeune",
    "prenom",
    "prenomme",
    "prenommee",
    "surnomme",
    "surnommee",
    "alias",
];

/// What sits between a word of kinship and the first names it announces:
/// `ses deux enfants mineurs, Léo et Maïa`.
const KIN_QUALIFIERS: [&str; 12] = [
    "mineur", "mineure", "mineurs", "mineures", "majeur", "majeure", "majeurs", "majeures", "aine",
    "ainee", "cadet", "cadette",
];

const PARTICLES: [&str; 12] = [
    "de", "du", "des", "le", "la", "van", "von", "der", "di", "da", "dit", "dite",
];

/// Words a decision writes in capitals without naming anyone: the vocabulary of
/// the procedure, the roles it distributes, and the towns where courts sit. A
/// city is not a person; whoever wants one gone types it.
const NOT_A_NAME: &[&str] = &[
    "abeille",
    "accorder",
    "acte",
    "administratif",
    "administrative",
    "affaire",
    "agen",
    "agence",
    "agent",
    "aide",
    "ainsi",
    "aix",
    "ajoutant",
    "alias",
    "alleged",
    "allianz",
    "allouer",
    "alors",
    "amenagement",
    "amiens",
    "and",
    "angers",
    "annexe",
    "annulation",
    "annule",
    "annulons",
    "anonyme",
    "aout",
    "appel",
    "appelant",
    "appelante",
    "appelantes",
    "appelants",
    "application",
    "apres",
    "aprf",
    "arret",
    "arrete",
    "arrondissement",
    "article",
    "ase",
    "asile",
    "assesseur",
    "assesseurs",
    "assistance",
    "assiste",
    "assistee",
    "association",
    "associes",
    "assurance",
    "assurances",
    "attaquee",
    "attendu",
    "attention",
    "au",
    "audience",
    "aussi",
    "autorise",
    "autorisee",
    "aux",
    "avant",
    "avec",
    "avisons",
    "aviva",
    "avocat",
    "avocate",
    "avocats",
    "avoir",
    "avons",
    "avril",
    "axa",
    "background",
    "banque",
    "bastia",
    "batonnier",
    "besancon",
    "bic",
    "bobigny",
    "bordeaux",
    "bourges",
    "brigade",
    "ca",
    "caa",
    "cabinet",
    "cadre",
    "caen",
    "caf",
    "caisse",
    "card",
    "carte",
    "case",
    "cassation",
    "casse",
    "cassons",
    "cause",
    "cayenne",
    "ccass",
    "ce",
    "cedex",
    "cedh",
    "cent",
    "centre",
    "ces",
    "ceseda",
    "cette",
    "ceux",
    "chambery",
    "chambre",
    "chapitre",
    "chef",
    "chefs",
    "cimade",
    "cinq",
    "cinquante",
    "civil",
    "civile",
    "cjue",
    "clinique",
    "cnil",
    "code",
    "colmar",
    "comite",
    "commerce",
    "commercial",
    "commettre",
    "commissaire",
    "commune",
    "compagnie",
    "comparant",
    "comparante",
    "composition",
    "concerning",
    "conclusions",
    "condamne",
    "condamner",
    "condamnons",
    "confie",
    "confirme",
    "confirmons",
    "conseil",
    "conseiller",
    "conseillere",
    "conseilleres",
    "conseillers",
    "considerant",
    "constate",
    "constatons",
    "constitutionnel",
    "contentieux",
    "contestation",
    "contradictoire",
    "contravention",
    "contre",
    "convention",
    "convocation",
    "cour",
    "cours",
    "court",
    "covea",
    "cpam",
    "cram",
    "cramif",
    "creance",
    "creteil",
    "criminel",
    "criminelle",
    "date",
    "debats",
    "debours",
    "deboute",
    "debouter",
    "deboutons",
    "decembre",
    "decide",
    "decides",
    "decidons",
    "decision",
    "declare",
    "declarer",
    "declarons",
    "deduire",
    "defenderesse",
    "defenderesses",
    "defendeur",
    "defendeurs",
    "defere",
    "deferee",
    "delegation",
    "delibere",
    "delivrance",
    "demande",
    "demanderesse",
    "demanderesses",
    "demandes",
    "demandeur",
    "demandeurs",
    "demeurant",
    "departement",
    "departemental",
    "deroulement",
    "des",
    "description",
    "designer",
    "designons",
    "detention",
    "deux",
    "deuxieme",
    "devant",
    "dijon",
    "dire",
    "direccte",
    "direction",
    "disant",
    "disons",
    "dispositif",
    "distincte",
    "district",
    "dit",
    "dite",
    "dix",
    "document",
    "domicilie",
    "domiciliee",
    "donc",
    "donnons",
    "dont",
    "dossier",
    "douai",
    "douze",
    "droit",
    "droits",
    "ecli",
    "educative",
    "egale",
    "elle",
    "elles",
    "eloignement",
    "en",
    "enfance",
    "enfants",
    "enjoignons",
    "enjoindre",
    "entre",
    "epouse",
    "et",
    "etat",
    "etranger",
    "etrangere",
    "etrangeres",
    "etrangers",
    "eurl",
    "europe",
    "europeen",
    "europeenne",
    "evry",
    "exces",
    "expert",
    "expertise",
    "expiry",
    "expose",
    "facts",
    "faisons",
    "fait",
    "faits",
    "federation",
    "fevrier",
    "final",
    "fixant",
    "fixer",
    "fixons",
    "fond",
    "fondamental",
    "fondamentales",
    "fonds",
    "for",
    "formation",
    "forme",
    "francais",
    "francaise",
    "francaises",
    "france",
    "frontiere",
    "garantie",
    "garde",
    "gendarmerie",
    "general",
    "generale",
    "generali",
    "gisti",
    "given",
    "gmf",
    "grande",
    "greffe",
    "greffier",
    "greffiere",
    "grenoble",
    "griefs",
    "groupama",
    "groupe",
    "height",
    "historique",
    "homme",
    "hommes",
    "homologuer",
    "hors",
    "huissier",
    "huit",
    "iard",
    "iban",
    "identite",
    "identity",
    "ils",
    "immobiliere",
    "indemnisation",
    "indiquons",
    "inferieur",
    "inferieure",
    "infirme",
    "infirmons",
    "informons",
    "infraction",
    "instance",
    "institut",
    "integral",
    "interesse",
    "interessee",
    "interieur",
    "interprete",
    "intervenant",
    "intime",
    "intimee",
    "intimees",
    "intimes",
    "introduction",
    "issue",
    "janvier",
    "judgment",
    "judicial",
    "juge",
    "jugement",
    "jugeons",
    "juger",
    "juges",
    "juillet",
    "juin",
    "juridiction",
    "juridique",
    "justice",
    "laissons",
    "law",
    "lecture",
    "legalite",
    "les",
    "leur",
    "leurs",
    "liberte",
    "libertes",
    "lieu",
    "lille",
    "limitee",
    "limiter",
    "limoges",
    "liquider",
    "litige",
    "localite",
    "location",
    "lodged",
    "loi",
    "lors",
    "lyon",
    "maaf",
    "macif",
    "macsf",
    "madame",
    "mademoiselle",
    "magistrat",
    "mai",
    "maif",
    "maine",
    "mais",
    "maitre",
    "majeur",
    "maladie",
    "mamoudzou",
    "mars",
    "marseille",
    "matmut",
    "maximale",
    "meaux",
    "medical",
    "medicale",
    "mesure",
    "metropole",
    "mettons",
    "metz",
    "mgen",
    "mille",
    "mineur",
    "mineure",
    "mineurs",
    "ministere",
    "ministre",
    "minute",
    "mma",
    "monsieur",
    "montpellier",
    "moteur",
    "motif",
    "motifs",
    "motivation",
    "motive",
    "motives",
    "moyen",
    "moyens",
    "msa",
    "mutualite",
    "mutuelle",
    "mutuelles",
    "naiss",
    "naissance",
    "names",
    "nancy",
    "nanterre",
    "nantes",
    "national",
    "nationale",
    "nationalite",
    "nationality",
    "nature",
    "ne",
    "nee",
    "neuf",
    "nice",
    "nimes",
    "nom",
    "non",
    "notice",
    "notification",
    "notre",
    "noumea",
    "nous",
    "nouveau",
    "novembre",
    "numero",
    "objet",
    "observations",
    "octobre",
    "of",
    "office",
    "ofpra",
    "oniam",
    "ont",
    "onze",
    "oqtf",
    "oqtn",
    "option",
    "ordonnance",
    "ordonnant",
    "ordonne",
    "ordonner",
    "ordonnons",
    "organisme",
    "orleans",
    "pacifica",
    "paiement",
    "papeete",
    "par",
    "paris",
    "parquet",
    "partie",
    "parties",
    "pau",
    "pays",
    "peuple",
    "piece",
    "pieces",
    "placement",
    "plier",
    "poitiers",
    "police",
    "pontoise",
    "portalis",
    "pour",
    "pourvoi",
    "pratique",
    "preambule",
    "precisons",
    "prefectoral",
    "prefecture",
    "prefet",
    "prefete",
    "prejudice",
    "prejudices",
    "premier",
    "premiere",
    "prenom",
    "prenoms",
    "president",
    "presidente",
    "pretentions",
    "preuve",
    "primaire",
    "prive",
    "privee",
    "procedurale",
    "procedure",
    "proceedings",
    "procureur",
    "prolongation",
    "prononce",
    "prononcons",
    "provence",
    "provision",
    "provisions",
    "prud",
    "public",
    "publics",
    "publique",
    "publiques",
    "qpc",
    "quarante",
    "quatorze",
    "quatre",
    "quatrieme",
    "que",
    "qui",
    "quinze",
    "quoi",
    "raid",
    "ramener",
    "rappel",
    "rappelle",
    "rappelons",
    "rapporteur",
    "rapporteure",
    "reasons",
    "recapitulatif",
    "recevons",
    "reconduite",
    "recours",
    "reformons",
    "reims",
    "rejeter",
    "rejetons",
    "rejette",
    "relatif",
    "relative",
    "rennes",
    "renvoyer",
    "renvoyons",
    "representant",
    "represente",
    "representee",
    "republique",
    "requerant",
    "requerante",
    "requerantes",
    "requerants",
    "requete",
    "requetes",
    "requisition",
    "reserver",
    "responsabilite",
    "restant",
    "resume",
    "retention",
    "retenu",
    "retenue",
    "retenues",
    "retenus",
    "review",
    "rg",
    "riom",
    "rouen",
    "rpva",
    "sa",
    "sans",
    "sante",
    "sarl",
    "sas",
    "sasu",
    "sauvegarde",
    "sci",
    "scp",
    "se",
    "second",
    "seconde",
    "secretariat",
    "section",
    "seize",
    "selarl",
    "selas",
    "semna",
    "sept",
    "septembre",
    "serenis",
    "service",
    "services",
    "ses",
    "sex",
    "sexe",
    "signature",
    "signe",
    "siren",
    "sirene",
    "siret",
    "situation",
    "smacl",
    "smic",
    "snc",
    "sncf",
    "sociale",
    "societe",
    "sogessur",
    "soixante",
    "solde",
    "sommaire",
    "son",
    "sont",
    "sous",
    "special",
    "speciale",
    "statuant",
    "statue",
    "statuer",
    "statuons",
    "strasbourg",
    "subsequent",
    "subsidiairement",
    "substitut",
    "succes",
    "sur",
    "surname",
    "surquoi",
    "surseoir",
    "sursis",
    "syndicat",
    "ta",
    "taille",
    "temoin",
    "tendant",
    "teneur",
    "territoire",
    "texte",
    "tfue",
    "tgi",
    "the",
    "these",
    "titre",
    "tj",
    "to",
    "total",
    "toulon",
    "toulouse",
    "tous",
    "tout",
    "toute",
    "toutes",
    "treize",
    "trente",
    "tribunal",
    "trois",
    "troisieme",
    "tue",
    "tutelle",
    "unanimously",
    "une",
    "union",
    "unite",
    "urssaf",
    "validite",
    "vehicule",
    "versailles",
    "veuve",
    "vice-president",
    "vice-presidente",
    "victimes",
    "ville",
    "vincennes",
    "vingt",
    "violation",
    "vitesse",
    "volontaire",
    "votre",
    "vous",
    "voyons",
    "vu",
    "vue",
    "with",
];

const STREETS: [&str; 26] = [
    "rue",
    "avenue",
    "boulevard",
    "impasse",
    "chemin",
    "place",
    "allee",
    "route",
    "quai",
    "cours",
    "square",
    "villa",
    "passage",
    "voie",
    "faubourg",
    "residence",
    "montee",
    "cite",
    "lotissement",
    "hameau",
    "immeuble",
    "sentier",
    "esplanade",
    "promenade",
    "clos",
    "bd",
];

/// Departments, regions and countries, hyphenated: a decision writes the
/// prefect's department and the applicant's country in capitals.
const PLACES: &[&str] = &[
    "afghanistan",
    "afrique",
    "ain",
    "aisne",
    "albanie",
    "algerie",
    "allemagne",
    "allier",
    "alpes-de-haute-provence",
    "alpes-maritimes",
    "alsace",
    "angola",
    "aquitaine",
    "arabie-saoudite",
    "ardeche",
    "ardennes",
    "argentine",
    "ariege",
    "armenie",
    "aube",
    "aude",
    "autriche",
    "auvergne",
    "auvergne-rhone-alpes",
    "aveyron",
    "azerbaidjan",
    "bangladesh",
    "bas-rhin",
    "belgique",
    "benin",
    "bielorussie",
    "birmanie",
    "bolivie",
    "bosnie",
    "bosnie-herzegovine",
    "bouches-du-rhone",
    "bourgogne",
    "bourgogne-franche-comte",
    "bresil",
    "bretagne",
    "bulgarie",
    "burkina",
    "burkina-faso",
    "burundi",
    "calvados",
    "cambodge",
    "cameroun",
    "canada",
    "cantal",
    "centrafrique",
    "centre-val-de-loire",
    "champagne",
    "charente",
    "charente-maritime",
    "cher",
    "chili",
    "chine",
    "chypre",
    "colombie",
    "comores",
    "congo",
    "coree",
    "correze",
    "corse-du-sud",
    "cote-d-ivoire",
    "cote-d-or",
    "cotes-d-armor",
    "creuse",
    "croatie",
    "cuba",
    "danemark",
    "deux-sevres",
    "djibouti",
    "dordogne",
    "doubs",
    "drome",
    "egypte",
    "emirats",
    "equateur",
    "erythree",
    "espagne",
    "essonne",
    "estonie",
    "ethiopie",
    "eure",
    "eure-et-loir",
    "finistere",
    "finlande",
    "gabon",
    "gambie",
    "gard",
    "georgie",
    "gers",
    "ghana",
    "gironde",
    "grand-est",
    "grece",
    "guadeloupe",
    "guinee",
    "guinee-bissau",
    "guinee-equatoriale",
    "guyane",
    "haiti",
    "haut-rhin",
    "haute-corse",
    "haute-garonne",
    "haute-loire",
    "haute-marne",
    "haute-saone",
    "haute-savoie",
    "haute-vienne",
    "hautes-alpes",
    "hautes-pyrenees",
    "hauts-de-france",
    "hauts-de-seine",
    "herault",
    "hongrie",
    "ile-de-france",
    "ille-et-vilaine",
    "inde",
    "indonesie",
    "indre",
    "indre-et-loire",
    "irak",
    "iran",
    "irlande",
    "isere",
    "islande",
    "israel",
    "italie",
    "jamaique",
    "japon",
    "jordanie",
    "jura",
    "kazakhstan",
    "kenya",
    "kosovo",
    "koweit",
    "landes",
    "laos",
    "lettonie",
    "liban",
    "liberia",
    "libye",
    "lituanie",
    "loir-et-cher",
    "loire",
    "loire-atlantique",
    "loiret",
    "lorraine",
    "lot",
    "lot-et-garonne",
    "lozere",
    "luxembourg",
    "macedoine",
    "madagascar",
    "maine-et-loire",
    "malaisie",
    "mali",
    "malte",
    "manche",
    "marne",
    "maroc",
    "martinique",
    "mauritanie",
    "mayenne",
    "mayotte",
    "meurthe-et-moselle",
    "meuse",
    "mexique",
    "moldavie",
    "mongolie",
    "montenegro",
    "morbihan",
    "moselle",
    "mozambique",
    "namibie",
    "nepal",
    "nievre",
    "niger",
    "nigeria",
    "nord",
    "normandie",
    "norvege",
    "nouvelle-aquitaine",
    "occitanie",
    "oise",
    "orne",
    "ouganda",
    "ouzbekistan",
    "pakistan",
    "palestine",
    "paraguay",
    "pas-de-calais",
    "pays-de-la-loire",
    "perou",
    "philippines",
    "picardie",
    "pologne",
    "portugal",
    "provence",
    "provence-alpes-cote-d-azur",
    "puy-de-dome",
    "pyrenees-atlantiques",
    "pyrenees-orientales",
    "qatar",
    "reunion",
    "rhone",
    "roumanie",
    "royaume-uni",
    "russie",
    "rwanda",
    "saone-et-loire",
    "sarthe",
    "savoie",
    "seine-et-marne",
    "seine-maritime",
    "seine-saint-denis",
    "senegal",
    "serbie",
    "sierra-leone",
    "slovaquie",
    "slovenie",
    "somalie",
    "somme",
    "soudan",
    "sri-lanka",
    "suede",
    "suisse",
    "syrie",
    "tadjikistan",
    "tanzanie",
    "tarn",
    "tarn-et-garonne",
    "tchad",
    "tchetchenie",
    "territoire-de-belfort",
    "thailande",
    "togo",
    "tunisie",
    "turquie",
    "ukraine",
    "uruguay",
    "val-d-oise",
    "val-de-marne",
    "var",
    "vaucluse",
    "vendee",
    "venezuela",
    "vienne",
    "vietnam",
    "vosges",
    "yemen",
    "yonne",
    "yvelines",
    "zambie",
    "zimbabwe",
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

/// One word of the source, as character bounds.
struct Word {
    start: usize,
    end: usize,
}

fn folded(chars: &[char]) -> String {
    let mut out = Vec::with_capacity(chars.len());
    for c in chars {
        fold_char(*c, &mut out);
    }
    out.into_iter().collect()
}

fn plain(chars: &[char], word: &Word) -> String {
    folded(&chars[word.start..word.end])
}

fn words(chars: &[char]) -> Vec<Word> {
    let part = |c: char| c.is_alphabetic() || c == '\'' || c == '-';
    let mut out = Vec::new();
    let mut start = None;
    for (index, c) in chars.iter().enumerate() {
        match (part(*c), start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                out.push(Word {
                    start: from,
                    end: index,
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push(Word {
            start: from,
            end: chars.len(),
        });
    }
    out.retain(|w| chars[w.start].is_alphabetic());
    out
}

fn capitalised(chars: &[char], word: &Word) -> bool {
    word.end - word.start >= 2 && chars[word.start].is_uppercase()
}

fn all_caps(chars: &[char], word: &Word) -> bool {
    word.end - word.start >= 2
        && chars[word.start..word.end]
            .iter()
            .all(|c| !c.is_alphabetic() || c.is_uppercase())
}

fn known(list: &[&str], probe: &str) -> bool {
    list.contains(&probe)
}

fn slice(chars: &[char], start: usize, end: usize) -> String {
    chars[start..end].iter().collect::<String>().trim().into()
}

/// What may sit between a civility and the name: the full stop of an
/// abbreviation, spacing, and one line break.
fn after_title(chars: &[char], left: &Word, right: &Word) -> bool {
    let gap = &chars[left.end..right.start];
    let stops = gap.iter().filter(|c| **c == '.').count();
    let breaks = gap.iter().filter(|c| **c == '\n').count();
    stops <= 1
        && breaks <= 1
        && (stops == 0 || gap.first() == Some(&'.'))
        && gap
            .iter()
            .all(|c| matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '.' | '\n'))
}

/// `J.` or `J.-C.` before a surname.
fn initial(chars: &[char], word: &Word, next: &Word) -> bool {
    word.end - word.start == 1
        && chars[word.start].is_uppercase()
        && chars.get(word.end) == Some(&'.')
        && chars[word.end..next.start]
            .iter()
            .all(|c| matches!(c, '.' | '-' | ' ') || c.is_uppercase())
}

/// A capitalised word that can be a first name or a surname.
fn name_word(chars: &[char], word: &Word) -> bool {
    let plain = plain(chars, word);
    capitalised(chars, word)
        && !common(chars, word)
        && !known(&CIVILITIES, &plain)
        && !known(&PARTICLES, &plain)
}

/// One name from `first` on: initials skipped, a particle kept inside it, and
/// the words of the procedure left out.
fn read_name(chars: &[char], words: &[Word], first: usize) -> Option<(usize, String)> {
    let mut start = first;
    while start + 1 < words.len()
        && start - first < 2
        && initial(chars, &words[start], &words[start + 1])
    {
        start += 1;
    }
    let mut last = None;
    let mut taken = 0;
    let mut at = start;
    loop {
        let word = &words[at];
        if name_word(chars, word) {
            last = Some(at);
            taken += 1;
        } else if !known(&PARTICLES, &plain(chars, word)) {
            break;
        }
        if taken == 3 || at + 1 >= words.len() || !joined(chars, word, &words[at + 1]) {
            break;
        }
        at += 1;
    }
    let last = last?;
    let text = slice(chars, words[start].start, words[last].end);
    nameable(&text).then_some((last, text))
}

/// A French form field, `Nom :`, `Nom de naissance :`, `Prénom(s) :`, `Nom et
/// prénom :`, and the value written after it on the same line, each given
/// name apart since the rest of the file writes them with the surname.
fn form_name(chars: &[char], words: &[Word], at: usize) -> Option<(usize, Vec<String>)> {
    #[derive(PartialEq)]
    enum Field {
        Surname,
        Given,
        Full,
    }
    let word = |i: usize| words.get(i).map(|w| plain(chars, w)).unwrap_or_default();
    let (field, label_end) = match (
        word(at).as_str(),
        word(at + 1).as_str(),
        word(at + 2).as_str(),
    ) {
        ("nom", "et", "prenom" | "prenoms") => (Field::Full, at + 2),
        ("nom", "de", "naissance" | "famille") => (Field::Surname, at + 2),
        ("nom", "d'usage" | "marital" | "patronymique", _) => (Field::Surname, at + 1),
        ("nom", _, _) => (Field::Surname, at),
        ("prenom" | "prenoms", "s", _) if chars[words[at].end] == '(' => (Field::Given, at + 1),
        ("prenom" | "prenoms", _, _) => (Field::Given, at),
        _ => return None,
    };
    let mut colon = words[label_end].end;
    while chars.get(colon).is_some_and(|c| matches!(c, ' ' | ')')) {
        colon += 1;
    }
    if chars.get(colon) != Some(&':') {
        return None;
    }
    let line_end = colon
        + chars[colon..]
            .iter()
            .position(|c| *c == '\n')
            .unwrap_or(chars.len() - colon);
    let first = label_end + 1;
    let fits = |w: &Word| {
        w.end <= line_end
            && w.end - w.start >= 3
            && legible(chars, w)
            && !common(chars, w)
            && !known(&CIVILITIES, &plain(chars, w))
    };
    let before: &[char] = &chars[colon + 1..words.get(first)?.start];
    if !fits(&words[first]) || before.iter().any(|c| c.is_alphanumeric()) {
        return None;
    }
    let mut last = first;
    while words.get(last + 1).is_some_and(|next| {
        let gap = &chars[words[last].end..next.start];
        fits(next)
            && gap.iter().all(|c| match field {
                Field::Given => matches!(c, ' ' | ','),
                _ => matches!(c, ' ' | '-'),
            })
    }) {
        last += 1;
    }
    let names = match field {
        Field::Given => words[first..=last]
            .iter()
            .map(|w| slice(chars, w.start, w.end))
            .collect(),
        Field::Surname | Field::Full => vec![slice(chars, words[first].start, words[last].end)],
    };
    Some((last, names))
}

/// `Je soussigné(e)` and the name that follows, a civility left to its own
/// rule.
fn undersigned(chars: &[char], words: &[Word], at: usize) -> Option<(usize, String)> {
    if !matches!(
        plain(chars, &words[at]).as_str(),
        "soussigne" | "soussignee" | "soussignes" | "soussignees"
    ) {
        return None;
    }
    let mut next = at + 1;
    if words
        .get(next)
        .is_some_and(|w| w.end - w.start == 1 && plain(chars, w) == "e")
    {
        next += 1;
    }
    if known(&CIVILITIES, &plain(chars, words.get(next)?)) {
        return None;
    }
    read_name(chars, words, next)
}

/// A civility, then the name that follows it, or the list a plural one opens.
fn after_civility(chars: &[char], words: &[Word], at: usize) -> Option<(usize, Vec<String>)> {
    let title = plain(chars, &words[at]);
    let title = title.trim_matches('-');
    if !(known(&CIVILITIES, title) || known(&FIRMS, title))
        || at + 1 >= words.len()
        || !after_title(chars, &words[at], &words[at + 1])
    {
        return None;
    }
    let (mut end, first) = read_name(chars, words, at + 1)?;
    let mut names = vec![first];
    if !(known(&PLURALS, title) || known(&FIRMS, title)) {
        return Some((end, names));
    }
    while end + 1 < words.len() {
        let gap: String = chars[words[end].end..words[end + 1].start].iter().collect();
        let mut next = end + 1;
        let comma = gap.trim() == ",";
        if !comma && !gap.trim().is_empty() {
            break;
        }
        if plain(chars, &words[next]) == "et" {
            next += 1;
        } else if !comma {
            break;
        }
        if next + 1 < words.len()
            && known(&CIVILITIES, &plain(chars, &words[next]))
            && after_title(chars, &words[next], &words[next + 1])
        {
            next += 1;
        }
        match words.get(next).and(read_name(chars, words, next)) {
            Some((last, name)) => {
                names.push(name);
                end = last;
            }
            None => break,
        }
    }
    Some((end, names))
}

/// The surname inside a full name: its words in capitals, or else all but the
/// first name.
pub fn surname_of(name: &str) -> Option<String> {
    let parts: Vec<&str> = name.split_whitespace().collect();
    if parts
        .first()
        .is_some_and(|p| p.starts_with(char::is_lowercase))
    {
        return None;
    }
    let capitals: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|p| p.chars().any(char::is_alphabetic) && !p.chars().any(char::is_lowercase))
        .collect();
    let surname = match capitals.is_empty() {
        true => parts.get(1..).unwrap_or_default().join(" "),
        false => capitals.join(" "),
    };
    (surname != name && nameable(&surname)).then_some(surname)
}

/// A first name the publisher kept beside the initial of the surname:
/// `Anselme B.`, `Lina X...`.
fn before_initial(chars: &[char], words: &[Word], at: usize) -> Option<(usize, String)> {
    let word = &words[at];
    let next = words.get(at + 1)?;
    if !name_word(chars, word)
        || all_caps(chars, word)
        || next.end - next.start != 1
        || !chars[next.start].is_uppercase()
        || !joined(chars, word, next)
    {
        return None;
    }
    let closes = match chars.get(next.end) {
        None => true,
        Some(c) => matches!(c, '.' | '\u{2026}' | ',' | ' ' | ')' | ';' | '\n'),
    };
    let text = slice(chars, word.start, word.end);
    (closes && nameable(&text)).then_some((at + 1, text))
}

/// First names after a word of kinship: `ses enfants Léo et Maïa`.
fn kin_names(chars: &[char], words: &[Word], at: usize) -> Option<(usize, Vec<String>)> {
    if !known(&KIN, &plain(chars, &words[at])) {
        return None;
    }
    let mut next = at + 1;
    while words.get(next).is_some_and(|w| {
        let word = plain(chars, w);
        (known(&PARTICLES, &word) || known(&KIN_QUALIFIERS, &word)) && !capitalised(chars, w)
    }) && next - at < 3
    {
        next += 1;
    }
    let mut names = Vec::new();
    let mut end = at;
    let mut previous = next - 1;
    while let Some(word) = words.get(next) {
        let gap: String = chars[words[previous].end..word.start].iter().collect();
        let gap = gap.trim();
        if !(gap.is_empty() || gap == ",") || !name_word(chars, word) {
            break;
        }
        let last = match read_name(chars, words, next) {
            Some((last, text)) => {
                names.push(text);
                last
            }
            None => next,
        };
        end = last;
        previous = last;
        next = last + 1;
        if words.get(next).is_some_and(|w| plain(chars, w) == "et") {
            previous = next;
            next += 1;
        }
    }
    (!names.is_empty()).then_some((end, names))
}

/// The bilingual field of an identity document, `NOM / Surname` or
/// `Prénoms / Given names`, whose value ends the line below: the reader leaves
/// the specimen's print in front of it, so only the tail of the line is read.
fn id_field(chars: &[char], words: &[Word], at: usize) -> Option<(usize, Vec<String>)> {
    let label = plain(chars, &words[at]);
    let english = words.get(at + 1).map(|w| plain(chars, w))?;
    let given = match (label.as_str(), english.as_str()) {
        ("prenom" | "prenoms", "given") => true,
        ("nom", "surname" | "sumame") => false,
        _ => return None,
    };
    let gap: String = chars[words[at].end..words[at + 1].start].iter().collect();
    if gap.trim() != "/" {
        return None;
    }
    let label_end = words[at + 1 + usize::from(given)].end;
    let break_at = label_end + chars[label_end..].iter().position(|c| *c == '\n')?;
    let next = words.iter().find(|w| w.start > break_at)?;
    let line_start = chars[..next.start]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |at| at + 1);
    let line_end = next.start
        + chars[next.start..]
            .iter()
            .position(|c| *c == '\n')
            .unwrap_or(chars.len() - next.start);
    let last = words
        .iter()
        .rposition(|w| w.end <= line_end && w.start >= line_start)?;
    let fits = |w: &Word| match given {
        true => name_word(chars, w) && legible(chars, w) && w.end - w.start >= 3,
        false => all_caps(chars, w) && !common(chars, w) && legible(chars, w),
    };
    let between = |left: &Word, right: &Word| {
        let gap: String = chars[left.end..right.start].iter().collect();
        match given {
            true => gap.trim() == ",",
            false => gap == " ",
        }
    };
    if !fits(&words[last]) {
        return None;
    }
    let mut first = last;
    while first > 0
        && words[first - 1].start >= line_start
        && fits(&words[first - 1])
        && between(&words[first - 1], &words[first])
    {
        first -= 1;
    }
    let names = match given {
        true => words[first..=last]
            .iter()
            .map(|w| slice(chars, w.start, w.end))
            .collect(),
        false => vec![slice(chars, words[first].start, words[last].end)],
    };
    Some((last, names))
}

/// A department, a region or a country, which a decision writes in capitals.
fn place_name(chars: &[char], words: &[Word], at: usize) -> Option<usize> {
    let mut key = String::new();
    let mut found = None;
    for end in at..(at + 4).min(words.len()) {
        if end > at && !joined(chars, &words[end - 1], &words[end]) {
            break;
        }
        let part = plain(chars, &words[end]).replace(['\'', '\u{2019}'], "-");
        let part = part
            .strip_prefix("l-")
            .or_else(|| part.strip_prefix("d-"))
            .filter(|_| end == at)
            .unwrap_or(&part)
            .to_string();
        if !key.is_empty() {
            key.push('-');
        }
        key.push_str(&part);
        if known(PLACES, &key) {
            found = Some(end);
        }
    }
    found
}

/// Under four letters, or made only of roman numerals, a run names a section
/// rather than a person. The search folds and matches inside words, so `ATI`
/// would light up every `systématiquement` of a report.
fn nameable(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 4
        && !letters
            .iter()
            .all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'))
}

/// Two words belong to the same name only when spacing separates them: a comma,
/// a colon or a full stop ends the run, whatever follows.
fn joined(chars: &[char], left: &Word, right: &Word) -> bool {
    chars[left.end..right.start]
        .iter()
        .all(|c| matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '-' | '\''))
}

/// A word a reader could have written: it has a vowel and no letter three
/// times over. A scan read badly yields `SSSR`, `NDPVF`, `TLRSS` in capitals.
fn legible(chars: &[char], word: &Word) -> bool {
    let plain = plain(chars, word);
    let letters: Vec<char> = plain.chars().filter(|c| c.is_alphabetic()).collect();
    let raw = &chars[word.start..word.end];
    letters.iter().any(|c| "aeiouy".contains(*c))
        && !letters.windows(3).any(|w| w[0] == w[1] && w[1] == w[2])
        && !raw
            .windows(2)
            .any(|w| w[0].is_lowercase() && w[1].is_uppercase())
}

/// `d'appel` is `appel` elided, and the list of what is not a name holds the
/// bare word; a compound is common when one of its parts is, as
/// `DEDICATED-OPTION`.
fn common(chars: &[char], word: &Word) -> bool {
    let plain = plain(chars, word);
    let bare = plain.rsplit('\'').next().unwrap_or(&plain);
    known(NOT_A_NAME, &plain)
        || known(NOT_A_NAME, bare)
        || (bare.contains('-') && bare.split('-').any(|part| known(NOT_A_NAME, part)))
}

/// A run of words in capitals, which is how a decision writes a surname, with
/// the first name written beside it on either side, and the surname alone too:
/// the rest of the document names the party by it. A particle joins a run it
/// sits inside, and never opens or closes one.
fn capitals_run(chars: &[char], words: &[Word], at: usize) -> Option<(usize, Vec<String>)> {
    let glued = |w: &Word| {
        chars.get(w.end).is_some_and(char::is_ascii_digit)
            || (w.start > 0 && chars[w.start - 1].is_ascii_digit())
    };
    let usable =
        |w: &Word| all_caps(chars, w) && !common(chars, w) && legible(chars, w) && !glued(w);
    let given = |w: &Word| {
        name_word(chars, w) && !all_caps(chars, w) && w.end - w.start >= 3 && legible(chars, w)
    };
    let mut first = at;
    let mut ahead = at;
    while ahead < at + 3
        && given(&words[ahead])
        && words
            .get(ahead + 1)
            .is_some_and(|next| joined(chars, &words[ahead], next))
    {
        ahead += 1;
    }
    if ahead > at && usable(&words[ahead]) {
        first = ahead;
    }
    if !usable(&words[first]) || known(&PARTICLES, &plain(chars, &words[first])) {
        return None;
    }
    let mut end = first;
    let mut last = first;
    while end + 1 < words.len() && end - first < 3 {
        let next = &words[end + 1];
        if !usable(next) || !joined(chars, &words[end], next) {
            break;
        }
        end += 1;
        if !known(&PARTICLES, &plain(chars, next)) {
            last = end;
        }
    }
    let surname_end = last;
    if first == at
        && words
            .get(last + 1)
            .is_some_and(|next| given(next) && joined(chars, &words[last], next))
    {
        last += 1;
    }
    // A heading word in capitals next to the run makes it a heading.
    let heading = |w: &Word| {
        all_caps(chars, w)
            && common(chars, w)
            && !known(&FIRMS, &plain(chars, w))
            && !matches!(plain(chars, w).as_str(), "et" | "republique" | "francaise")
    };
    // Two spaces part the cells of a header, which a heading does not cross.
    let tight =
        |left: &Word, right: &Word| right.start - left.end == 1 && joined(chars, left, right);
    let before = first
        .checked_sub(1)
        .map(|i| &words[i])
        .is_some_and(|w| heading(w) && tight(w, &words[first]));
    let after = words
        .get(end + 1)
        .is_some_and(|w| heading(w) && tight(&words[end], w));
    if before || after {
        return None;
    }
    // Two-letter words make half of what a scan read badly: `RE ER EE`.
    let (short, total) = words[at..=last]
        .iter()
        .filter(|w| !known(&PARTICLES, &plain(chars, w)))
        .fold((0, 0), |(short, total), w| {
            (short + usize::from(w.end - w.start <= 2), total + 1)
        });
    if 2 * short > total {
        return None;
    }
    let text = slice(chars, words[at].start, words[last].end);
    if !nameable(&text) {
        return None;
    }
    let surname = slice(chars, words[first].start, words[surname_end].end);
    let mut names = vec![text];
    if (first != at || surname_end != last) && nameable(&surname) {
        names.push(surname);
    }
    Some((last, names))
}

/// Five digits in a row, which is how a French address ends before its city.
fn postcode(text: &[char]) -> Option<usize> {
    let mut run = 0;
    for (index, c) in text.iter().enumerate() {
        match c.is_ascii_digit() {
            true => run += 1,
            false if run == 5 => return Some(index),
            false => run = 0,
        }
    }
    (run == 5).then_some(text.len())
}

/// Words after which a decision writes where a person lives.
const RESIDES: [&str; 19] = [
    "vecu",
    "vivre",
    "demeurant",
    "demeurante",
    "demeurants",
    "demeure",
    "domicilie",
    "domiciliee",
    "domicilies",
    "domiciliees",
    "residant",
    "residante",
    "reside",
    "residait",
    "heberge",
    "hebergee",
    "habitant",
    "habitante",
    "vivant",
];

/// The end of the town a postcode opens, `13170 LES PENNES MIRABEAU`, or of
/// the postcode a town closes with, `à Cambrai (59400)`.
fn after_postcode(chars: &[char], words: &[Word], code_end: usize) -> usize {
    if chars.get(code_end) == Some(&')') {
        return code_end + 1;
    }
    let Some(at) = words.iter().position(|w| w.start >= code_end) else {
        return code_end;
    };
    let gap = &chars[code_end..words[at].start];
    if !gap.iter().all(|c| matches!(c, ' ' | '\u{a0}' | '-')) || !capitalised(chars, &words[at]) {
        return code_end;
    }
    let mut last = at;
    while last + 1 < words.len()
        && last - at < 4
        && joined(chars, &words[last], &words[last + 1])
        && words[last + 1].end - words[last + 1].start >= 2
        && chars[words[last + 1].start].is_uppercase()
    {
        last += 1;
    }
    words[last].end
}

/// Where a person lives, read from the word that says so: the address as far
/// as the town after its postcode, `demeurant Résidence les Pins, bâtiment
/// A2, 13240 SEPTÈMES`, or the town alone, `demeurant à Vénissieux`.
fn residence(chars: &[char], words: &[Word], at: usize) -> Option<(usize, String)> {
    let head = plain(chars, &words[at]);
    let elected = head == "elisant"
        && words
            .get(at + 1)
            .is_some_and(|w| plain(chars, w) == "domicile");
    if !known(&RESIDES, &head) && !elected {
        return None;
    }
    let mut next = at + 1 + usize::from(elected);
    // `demeurant et domicilié`
    if words.get(next).is_some_and(|w| plain(chars, w) == "et")
        && words
            .get(next + 1)
            .is_some_and(|w| known(&RESIDES, &plain(chars, w)))
    {
        next += 2;
    }
    let mut start = words[next - 1].end;
    while start < chars.len() && matches!(chars[start], ' ' | ',' | ':' | '\u{a0}') {
        start += 1;
    }
    let word_at = |from: usize| words.iter().position(|w| w.start >= from);
    // `à Lyon` without a street: the town, as far as its postcode.
    if let Some(i) = word_at(start).filter(|&i| words[i].start == start) {
        if matches!(plain(chars, &words[i]).as_str(), "a" | "au") {
            let (last, text) = town(chars, words, i + 1)?;
            let mut end = words[last].end;
            let mut text = text;
            let mut probe = end;
            while chars.get(probe) == Some(&' ') {
                probe += 1;
            }
            if chars.get(probe) == Some(&'(') {
                if let Some(close) = (probe..chars.len().min(probe + 8)).find(|&c| chars[c] == ')')
                {
                    if postcode(&chars[probe + 1..close]).is_some() {
                        end = close + 1;
                        text = slice(chars, words[i + 1].start, end);
                    }
                }
            }
            let last = words.iter().rposition(|w| w.end <= end).unwrap_or(last);
            return Some((last, text));
        }
    }
    // An address carries its postcode before the line ends, or on the next.
    let limit = chars.len().min(start + 180);
    let mut lines = 0;
    let mut run = 0;
    for i in start..limit {
        let c = chars[i];
        if c == '\n' {
            lines += 1;
            if lines == 2 {
                return None;
            }
        }
        if c == ';' {
            return None;
        }
        match c.is_ascii_digit() {
            true => run += 1,
            false if run == 5 => {
                let end = after_postcode(chars, words, i);
                let text = slice(chars, start, end);
                let last = words.iter().rposition(|w| w.end <= end)?;
                return (last > at && text.chars().any(char::is_alphabetic))
                    .then_some((last, text));
            }
            false => run = 0,
        }
    }
    None
}

/// A line of two to five capitalised words set right above an address: the
/// addressee block of a letter or an invoice.
fn addressee(chars: &[char], words: &[Word], at: usize) -> Option<(usize, String)> {
    let line_start = words[at].start;
    if line_start > 0 && chars[line_start - 1] != '\n' {
        return None;
    }
    let line_end = line_start
        + chars[line_start..]
            .iter()
            .position(|c| *c == '\n')
            .unwrap_or(chars.len() - line_start);
    let mut last = at;
    while words.get(last + 1).is_some_and(|w| w.end <= line_end) {
        last += 1;
    }
    let count = last + 1 - at;
    let fits = |w: &Word| name_word(chars, w) && legible(chars, w);
    let spaced = words[at..last]
        .iter()
        .zip(&words[at + 1..=last])
        .all(|(left, right)| joined(chars, left, right));
    let tail: String = chars[words[last].end..line_end].iter().collect();
    if !(2..=5).contains(&count)
        || !words[at..=last].iter().all(fits)
        || !spaced
        || !tail.trim().is_empty()
    {
        return None;
    }
    let next = words.get(last + 1)?;
    let between = &chars[line_end..next.start];
    let lines = between.iter().filter(|c| **c == '\n').count();
    if lines > 2
        || between
            .iter()
            .any(|c| !c.is_whitespace() && !c.is_ascii_digit())
    {
        return None;
    }
    address(chars, words, last + 1)?;
    Some((last, slice(chars, line_start, words[last].end)))
}

/// A house number, a street word, and everything up to the city.
fn address(chars: &[char], words: &[Word], at: usize) -> Option<(usize, String)> {
    if !known(&STREETS, &plain(chars, &words[at])) {
        return None;
    }
    let mut start = words[at].start;
    while start > 0 && chars[start - 1] == ' ' {
        start -= 1;
    }
    if start > 1 && chars[start - 1] == ',' && chars[start - 2].is_ascii_digit() {
        start -= 1;
    }
    while start > 0 && (chars[start - 1] == ' ' || chars[start - 1].is_ascii_digit()) {
        start -= 1;
    }
    if !chars[start..words[at].start]
        .iter()
        .any(char::is_ascii_digit)
    {
        return None;
    }
    let mut end = at;
    while end + 1 < words.len() && end - at < 8 {
        end += 1;
        let gap = &chars[words[end - 1].end..words[end].start];
        if let Some(after) = postcode(gap) {
            // `75011 Paris` takes the city, `à Lyon (69003), par` stops on the code.
            let rest = &gap[after..];
            return match rest.iter().any(|c| matches!(c, ')' | ',' | ';' | '\n')) {
                true => {
                    let close =
                        words[end - 1].end + after + usize::from(rest.first() == Some(&')'));
                    Some((end - 1, slice(chars, start, close)))
                }
                false => Some((end, slice(chars, start, words[end].end))),
            };
        }
        if gap.contains(&';') {
            end -= 1;
            break;
        }
        let ahead = words
            .get(end + 4)
            .map_or(chars.len(), |w| w.start)
            .min(chars.len());
        if gap.contains(&',')
            && end - at > 2
            && postcode(&chars[words[end - 1].end..ahead]).is_none()
        {
            end -= 1;
            break;
        }
        // `rue Madame` is a street, `rue des Lilas et Madame Vassort` two parties.
        if end - at > 1 && known(&CIVILITIES, &plain(chars, &words[end])) {
            end -= 1;
            if plain(chars, &words[end]) == "et" {
                end -= 1;
            }
            break;
        }
    }
    Some((end, slice(chars, start, words[end].end)))
}

/// Capitalised words after `à`, as far as a punctuation mark. A word inside
/// brackets is a label someone already put there: `à [Localité 6]`.
fn town(chars: &[char], words: &[Word], at: usize) -> Option<(usize, String)> {
    let first = words.get(at)?;
    // A blank line or a form's asterisk parts a label from the next field.
    let apart = at > 0 && {
        let gap = &chars[words[at - 1].end..first.start];
        gap.iter().filter(|c| **c == '\n').count() > 1 || gap.contains(&'*')
    };
    if apart
        || !capitalised(chars, first)
        || known(&CIVILITIES, &plain(chars, first))
        || (first.start > 0 && chars[first.start - 1] == '[')
    {
        return None;
    }
    let mut last = at;
    while last + 1 < words.len()
        && last - at < 3
        && joined(chars, &words[last], &words[last + 1])
        && capitalised(chars, &words[last + 1])
        && !common(chars, &words[last + 1])
    {
        last += 1;
    }
    let text = slice(chars, first.start, words[last].end);
    (text.chars().filter(|c| c.is_alphabetic()).count() >= 3).then_some((last, text))
}

/// What a parenthesis right after `end` holds: `(06)`, `(Guinée)`.
fn bracketed(chars: &[char], end: usize) -> Option<(usize, String)> {
    let mut open = end;
    while chars.get(open) == Some(&' ') {
        open += 1;
    }
    if chars.get(open) != Some(&'(') {
        return None;
    }
    let close = (open + 1..chars.len().min(open + 40)).find(|&i| chars[i] == ')')?;
    let inside = slice(chars, open + 1, close);
    (!inside.is_empty() && !inside.contains('\n')).then_some((close + 1, inside))
}

const WEEKDAYS: [&str; 7] = [
    "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
];

/// The date of birth that starts at `from`: `12/03/1970`, `3 mars 1970`,
/// `1er juin 1985`, `janvier 2004`, a year alone, and the year a publisher
/// left after its label, `[Date naissance 1] 1998`. A weekday before it is
/// skipped, and so is what a reader makes of `1er`: `1”`, `1°`.
fn date_at(chars: &[char], from: usize) -> Option<(usize, usize)> {
    let skip = |at: &mut usize| {
        while *at < chars.len() && chars[*at] == ' ' {
            *at += 1;
        }
    };
    let mut start = from;
    skip(&mut start);
    if chars.get(start) == Some(&'[') {
        start = (start..chars.len().min(start + 40)).find(|&i| chars[i] == ']')? + 1;
        skip(&mut start);
    }
    let mut word = start;
    while word < chars.len() && chars[word].is_alphabetic() {
        word += 1;
    }
    if known(&WEEKDAYS, &folded(&chars[start..word])) {
        start = word;
        skip(&mut start);
    }
    let mut end = start;
    let digits = |end: &mut usize| {
        while *end < chars.len()
            && (chars[*end].is_ascii_digit() || chars[*end] == ' ' || chars[*end] == '/')
        {
            *end += 1;
        }
    };
    digits(&mut end);
    let day = chars[start..end].iter().any(char::is_ascii_digit);
    if day && chars[end..].starts_with(&['e', 'r']) {
        end += 2;
        digits(&mut end);
    } else if day && matches!(chars.get(end), Some('”' | '"' | '°' | '’' | '*')) {
        while matches!(chars.get(end), Some('”' | '"' | '°' | '’' | '*')) && end - start < 5 {
            end += 1;
        }
        digits(&mut end);
    }
    let mut month = end;
    while month < chars.len() && chars[month].is_alphabetic() {
        month += 1;
    }
    let named = known(&MONTHS, &folded(&chars[end..month]));
    if named {
        end = month;
        while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == ' ') {
            end += 1;
        }
    }
    let piece = &chars[start..end];
    let count = piece.iter().filter(|c| c.is_ascii_digit()).count();
    let year: String = piece.iter().filter(|c| !c.is_whitespace()).collect();
    let year_alone = year.len() == 4
        && year
            .parse::<u32>()
            .is_ok_and(|y| (1880..=2040).contains(&y));
    (count >= 5 || (named && count >= 4) || year_alone).then_some((start, end))
}

/// `natif de Kayes`, `date de naissance : 12/03/1970`, `lieu de naissance :
/// Conakry`, `sa date de naissance le 22 décembre 1994 à Mwilambongo`.
fn born_at(
    chars: &[char],
    words: &[Word],
    at: usize,
) -> Option<(usize, Option<String>, Vec<String>)> {
    let next = words.get(at + 1)?;
    let gap: String = chars[words[at].end..next.start].iter().collect();
    let word = plain(chars, next);
    if plain(chars, &words[at]) != "naissance" {
        let (last, text) = (word == "de")
            .then(|| town(chars, words, at + 2))
            .flatten()?;
        return Some((last, None, vec![text]));
    }
    if gap.trim() == ":" {
        if let Some((start, end)) = date_at(chars, words[at].end + gap.len() - 1) {
            let last = words.iter().rposition(|w| w.end <= end).unwrap_or(at);
            return Some((last.max(at), Some(slice(chars, start, end)), Vec::new()));
        }
        let (last, text) = town(chars, words, at + 1)?;
        return Some((last, None, vec![text]));
    }
    let mut date = None;
    let mut places = Vec::new();
    let mut cursor = at + 1;
    let mut last = at;
    while let Some(w) = words.get(cursor) {
        match plain(chars, w).as_str() {
            "le" if date.is_none() => {
                let (start, end) = date_at(chars, w.end)?;
                date = Some(slice(chars, start, end));
                while cursor + 1 < words.len() && words[cursor + 1].start < end {
                    cursor += 1;
                }
            }
            "a" if places.is_empty() => {
                let (town_end, text) = town(chars, words, cursor + 1)?;
                places.push(text);
                cursor = town_end;
            }
            _ => break,
        }
        last = cursor;
        cursor += 1;
    }
    (last > at).then_some((last, date, places))
}

/// `né le 12/03/1970 à Lyon`, `née à Lyon le 3 mars 1970`, `né en 1996`, `né
/// le 20 janvier 2000 au Mali`: the date and the places written with a birth,
/// whichever of them the sentence carries.
fn birth(
    chars: &[char],
    words: &[Word],
    at: usize,
) -> Option<(usize, Option<String>, Vec<String>)> {
    let head = plain(chars, &words[at]);
    let name_label =
        at >= 2 && plain(chars, &words[at - 2]) == "nom" && plain(chars, &words[at - 1]) == "de";
    if head == "naissance" && name_label {
        return None;
    }
    if matches!(head.as_str(), "natif" | "native" | "naissance") {
        return born_at(chars, words, at);
    }
    if !matches!(head.as_str(), "ne" | "nee" | "nes" | "nees") {
        return None;
    }
    // `ne` without its accent is the negation as often as a reader's `né`:
    // only a date makes it a birth.
    let accented = chars[words[at].start + 1] != 'e' && chars[words[at].start + 1] != 'E';
    let first = at + 1;
    let mut cursor = at + 1;
    if cursor < words.len() && plain(chars, &words[cursor]) == "e" {
        cursor += 1;
    }
    let mut date = None;
    let mut places = Vec::new();
    let mut last = at;
    let mut hops = 0;
    // Up to four words before the first part, none between two parts, nor
    // before the date an unaccented `ne` needs.
    while cursor < words.len()
        && hops <= if last == at && accented { 4 } else { 0 }
        && (date.is_none() || places.is_empty())
    {
        let word = plain(chars, &words[cursor]);
        let mut matched = false;
        if matches!(word.as_str(), "le" | "en") && date.is_none() {
            let mut from = words[cursor].end;
            while chars.get(from) == Some(&' ') {
                from += 1;
            }
            if chars.get(from) == Some(&':') {
                from += 1;
            }
            if let Some((start, end)) = date_at(chars, from) {
                date = Some(slice(chars, start, end));
                while cursor + 1 < words.len() && words[cursor + 1].start < end {
                    cursor += 1;
                }
                matched = true;
            }
        }
        let country =
            matches!(word.as_str(), "au" | "aux" | "en") && (cursor == first || last > at);
        if !matched && (word == "a" || country) && places.is_empty() && (accented || date.is_some())
        {
            if let Some((town_end, text)) = town(chars, words, cursor + 1) {
                let mut end_char = words[town_end].end;
                places.push(text);
                if let Some((close, _)) = bracketed(chars, end_char) {
                    end_char = close;
                }
                cursor = town_end;
                while cursor + 1 < words.len() && words[cursor + 1].start < end_char {
                    cursor += 1;
                }
                matched = true;
            }
        }
        match matched {
            true => {
                last = cursor;
                hops = 0;
            }
            false => hops += 1,
        }
        cursor += 1;
    }
    (date.is_some() || !places.is_empty()).then_some((last, date, places))
}

fn email(chars: &[char], at: usize) -> Option<(usize, String)> {
    if chars[at] != '@' {
        return None;
    }
    let local = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    let domain = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    let mut start = at;
    while start > 0 && local(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && domain(chars[end]) {
        end += 1;
    }
    let text: String = chars[start..end].iter().collect();
    let (before, after) = text.split_once('@')?;
    match !before.is_empty() && after.contains('.') && after.len() >= 4 {
        true => Some((end, text.trim_end_matches('.').to_string())),
        false => None,
    }
}

/// A run of digits, with the separators a French number is written with.
fn digit_run(chars: &[char], at: usize) -> (usize, usize) {
    let mut end = at;
    let mut digits = 0;
    while end < chars.len() {
        let c = chars[end];
        if c.is_ascii_digit() {
            digits += 1;
            end += 1;
        } else if " .-/".contains(c) && end + 1 < chars.len() && chars[end + 1].is_ascii_digit() {
            end += 1;
        } else {
            break;
        }
    }
    (end, digits)
}

fn numeric(chars: &[char], at: usize) -> Option<(usize, Kind, String)> {
    if !chars[at].is_ascii_digit() || (at > 0 && chars[at - 1].is_ascii_digit()) {
        return None;
    }
    let (end, digits) = digit_run(chars, at);
    let text = slice(chars, at, end);
    let bare: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits == 10 && bare.starts_with('0') && !bare.starts_with("00") {
        return Some((end, Kind::Phone, text));
    }
    if digits == 13 && (bare.starts_with('1') || bare.starts_with('2')) {
        return Some((end, Kind::Identifier, text));
    }
    if digits == 15
        && (bare.starts_with('1') || bare.starts_with('2'))
        && (nir_key_holds(&bare) || insured(chars, at))
    {
        return Some((end, Kind::Identifier, text));
    }
    if (digits == 9 || digits == 14) && luhn(&bare) {
        return Some((end, Kind::Identifier, text));
    }
    if text.contains('/')
        && (6..=12).contains(&digits)
        && !cited_text(chars, at)
        && !calendar(&text)
    {
        return Some((end, Kind::Docket, text));
    }
    None
}

/// Whether the words before `at` announce a social security number, whose key
/// a decision sometimes misprints.
fn insured(chars: &[char], at: usize) -> bool {
    let before = folded(&chars[at.saturating_sub(80)..at]);
    [
        "securite sociale",
        "immatricul",
        "insee",
        " nir",
        "n° ss",
        "assure social",
    ]
    .iter()
    .any(|w| before.contains(w))
}

/// The key of a social security number is 97 minus its first thirteen digits modulo 97.
fn nir_key_holds(bare: &str) -> bool {
    let (body, key) = bare.split_at(13);
    let body: u64 = body.parse().expect("thirteen digits");
    let key: u64 = key.parse().expect("two digits");
    97 - body % 97 == key
}

/// SIREN and SIRET carry a Luhn check digit.
fn luhn(bare: &str) -> bool {
    let sum: u32 = bare
        .bytes()
        .rev()
        .enumerate()
        .map(|(rank, b)| {
            let digit = u32::from(b - b'0');
            match rank % 2 {
                1 if digit > 4 => digit * 2 - 9,
                1 => digit * 2,
                _ => digit,
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// `FR76 3000 6000 0112 3456 7890 189`: the longest run of groups whose
/// ISO 13616 check holds, so a capitalised word after it is left out.
fn iban(chars: &[char], at: usize) -> Option<(usize, String)> {
    let head = chars.get(at..at + 4)?;
    if (at > 0 && chars[at - 1].is_alphanumeric())
        || !head[..2].iter().all(char::is_ascii_uppercase)
        || !head[2..].iter().all(char::is_ascii_digit)
    {
        return None;
    }
    let alnum = |c: &char| c.is_ascii_digit() || c.is_ascii_uppercase();
    let mut code: String = head.iter().collect();
    let mut end = at + 4;
    let mut found = None;
    loop {
        if (15..=34).contains(&code.len()) && iban_check_holds(&code) {
            found = Some((end, slice(chars, at, end)));
        }
        let start = match chars.get(end) {
            Some(' ') => end + 1,
            Some(c) if alnum(c) => end,
            _ => break,
        };
        let group_end = (start..chars.len())
            .find(|&i| !alnum(&chars[i]))
            .unwrap_or(chars.len());
        if group_end == start || code.len() + group_end - start > 34 {
            break;
        }
        code.extend(&chars[start..group_end]);
        end = group_end;
    }
    found
}

/// A registration plate: `AA-593-GV` as France numbers them since 2009, or
/// whatever follows `immatriculé`, letters and digits together, `WW332ST`.
fn plate(chars: &[char], at: usize) -> Option<(usize, String)> {
    if at > 0 && chars[at - 1].is_alphanumeric() || !chars[at].is_ascii_alphanumeric() {
        return None;
    }
    let token_end = (at..chars.len())
        .find(|&i| !(chars[i].is_ascii_alphanumeric() || chars[i] == '-'))
        .unwrap_or(chars.len());
    let token: Vec<char> = chars[at..token_end].to_vec();
    let bare: Vec<char> = token.iter().copied().filter(|c| *c != '-').collect();
    let siv = bare.len() == 7
        && bare[..2].iter().all(char::is_ascii_uppercase)
        && bare[2..5].iter().all(char::is_ascii_digit)
        && bare[5..].iter().all(char::is_ascii_uppercase)
        && (token.len() == 7 || (token.len() == 9 && token[2] == '-' && token[6] == '-'));
    let announced = (5..=10).contains(&bare.len())
        && bare.iter().any(char::is_ascii_digit)
        && bare.iter().any(char::is_ascii_uppercase)
        && bare
            .iter()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && {
            let before = folded(&chars[at.saturating_sub(16)..at]);
            before.trim_end().ends_with("immatricule")
                || before.trim_end().ends_with("immatriculee")
        };
    (siv || announced).then(|| (token_end, token.iter().collect()))
}

fn iban_check_holds(code: &str) -> bool {
    let (country, rest) = code.split_at(4);
    rest.chars().chain(country.chars()).fold(0u32, |acc, c| {
        let value = c.to_digit(36).expect("checked alphanumeric");
        match value {
            0..=9 => (acc * 10 + value) % 97,
            _ => (acc * 100 + value) % 97,
        }
    }) == 1
}

/// `directive 2008/115`, `règlement (UE) n° 604/2013`: a text, public, not a case.
fn cited_text(chars: &[char], at: usize) -> bool {
    let before = folded(&chars[at.saturating_sub(32)..at]);
    [
        "directive",
        "reglement",
        "decret",
        "loi ",
        "convention",
        "decision-cadre",
    ]
    .iter()
    .any(|text| before.contains(text))
}

/// `28/11/2016` is a date and `2014/2015` a school year.
fn calendar(text: &str) -> bool {
    let parts: Vec<&str> = text.split('/').map(str::trim).collect();
    let numbers: Vec<Option<u32>> = parts.iter().map(|p| p.parse().ok()).collect();
    match numbers[..] {
        [Some(day), Some(month), Some(_)] => {
            parts[2].len() == 4 && (1..=31).contains(&day) && (1..=12).contains(&month)
        }
        [Some(first), Some(second)] => parts[0].len() == 4 && second == first + 1,
        _ => false,
    }
}

fn push(out: &mut Vec<Suggestion>, kind: Kind, text: String) {
    let text = text.trim_matches(|c: char| c == ',' || c == '.' || c.is_whitespace());
    if text.chars().count() < 3 {
        return;
    }
    match out
        .iter_mut()
        .find(|s| s.kind == kind && s.text.eq_ignore_ascii_case(text))
    {
        Some(seen) => seen.count += 1,
        None => out.push(Suggestion {
            text: text.to_string(),
            kind,
            count: 1,
        }),
    }
}

/// Everything the rules find, most frequent first inside each kind.
pub fn suggest(source: &[(char, Anchor)]) -> Vec<Suggestion> {
    let chars: Vec<char> = source.iter().map(|(c, _)| *c).collect();
    let words = words(&chars);
    let mut out = Vec::new();

    let mut index = 0;
    while index < words.len() {
        if let Some((end, names)) = id_field(&chars, &words, index) {
            for name in names {
                push(&mut out, Kind::Person, name);
            }
            index = end + 1;
            continue;
        }
        if let Some((end, names)) = form_name(&chars, &words, index) {
            for name in names {
                push(&mut out, Kind::Person, name);
            }
            index = end + 1;
            continue;
        }
        if let Some((end, name)) = undersigned(&chars, &words, index) {
            let surname = surname_of(&name);
            push(&mut out, Kind::Person, name);
            if let Some(surname) = surname {
                push(&mut out, Kind::Person, surname);
            }
            index = end + 1;
            continue;
        }
        if let Some((end, names)) = after_civility(&chars, &words, index) {
            for name in names {
                let surname = surname_of(&name);
                push(&mut out, Kind::Person, name);
                if let Some(surname) = surname {
                    push(&mut out, Kind::Person, surname);
                }
            }
            index = end + 1;
            continue;
        }
        if let Some((end, date, places)) = birth(&chars, &words, index) {
            if let Some(date) = date {
                push(&mut out, Kind::BirthDate, date);
            }
            for place in places {
                push(&mut out, Kind::BirthPlace, place);
            }
            index = end + 1;
            continue;
        }
        if let Some((end, name)) = addressee(&chars, &words, index) {
            push(&mut out, Kind::Person, name);
            index = end + 1;
            continue;
        }
        if let Some((end, text)) = residence(&chars, &words, index) {
            push(&mut out, Kind::Address, text);
            index = end + 1;
            continue;
        }
        if let Some((end, text)) = address(&chars, &words, index) {
            push(&mut out, Kind::Address, text);
            index = end + 1;
            continue;
        }
        if let Some((end, names)) = kin_names(&chars, &words, index) {
            for name in names {
                push(&mut out, Kind::Person, name);
            }
            index = end + 1;
            continue;
        }
        if let Some(end) = place_name(&chars, &words, index) {
            index = end + 1;
            continue;
        }
        if let Some((end, text)) = before_initial(&chars, &words, index) {
            push(&mut out, Kind::Person, text);
            index = end + 1;
            continue;
        }
        if let Some((end, names)) = capitals_run(&chars, &words, index) {
            for name in names {
                push(&mut out, Kind::Person, name);
            }
            index = end + 1;
            continue;
        }
        index += 1;
    }

    let mut cursor = 0;
    while cursor < chars.len() {
        if let Some((end, text)) = email(&chars, cursor) {
            push(&mut out, Kind::Email, text);
            cursor = end;
            continue;
        }
        if let Some((end, text)) = iban(&chars, cursor) {
            push(&mut out, Kind::Identifier, text);
            cursor = end;
            continue;
        }
        if let Some((end, text)) = plate(&chars, cursor) {
            push(&mut out, Kind::Identifier, text);
            cursor = end;
            continue;
        }
        if let Some((end, kind, text)) = numeric(&chars, cursor) {
            push(&mut out, kind, text);
            cursor = end;
            continue;
        }
        cursor = match chars[cursor].is_ascii_digit() {
            true => digit_run(&chars, cursor).0,
            false => cursor + 1,
        };
    }

    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(b.count.cmp(&a.count)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tests::glyphs as source;

    fn of(kind: Kind, text: &str) -> Vec<Suggestion> {
        suggest(&source(text))
            .into_iter()
            .filter(|s| s.kind == kind)
            .collect()
    }

    #[test]
    fn a_civility_carries_the_name_that_follows_it() {
        let found = of(
            Kind::Person,
            "à la requête de Monsieur Jean Dupont, demeurant",
        );
        assert_eq!(found[0].text, "Jean Dupont");
    }

    #[test]
    fn an_identity_card_read_by_eye_gives_its_names_and_not_its_labels() {
        let card = "CARTE NATIONALE D'IDENTITÉ / IDENTITY CARD\n\nNOM / Sumame\n\nAUS EC \\ DUVAL-MARTIN\n\n, Î4Y | ga: Prénoms / Given names\n\nA Ée LE 4 Léa, Marguerite, Anaïs\n\nSEXE /Sex NATIONALITÉ / Nationality\n\nLIEU DE NAISSANCE / Place ofbirh\n\nLYON 3E ARRONDISSEMENT\n\nIDFRAXQZ8KVN316<<<<<<<<";
        let found: Vec<String> = of(Kind::Person, card).into_iter().map(|s| s.text).collect();
        for name in ["DUVAL-MARTIN", "Léa", "Marguerite", "Anaïs"] {
            assert!(
                found.contains(&name.to_string()),
                "{name} missing from {found:?}"
            );
        }
        for label in [
            "IDENTITY CARD",
            "SEXE",
            "LIEU DE NAISSANCE",
            "ARRONDISSEMENT",
            "IDFRAXQZ",
            "AUS EC",
        ] {
            assert!(
                !found.contains(&label.to_string()),
                "{label} proposed in {found:?}"
            );
        }
    }

    #[test]
    fn an_invoice_gives_its_addressee_and_not_its_product_words() {
        let invoice = "Léa Duval Martin\n\n12 rue des Lilas\n\n75011 PARIS\n\nRubrique divers DEDICATED-OPTION\n\n2x 500Gb SSD NVMe Soft RAID rental for 1 month\n\nIBAN : FR76 3000 6000 0112 3456 7890 189";
        let found = texts(Kind::Person, invoice);
        assert_eq!(found, vec!["Léa Duval Martin"]);
    }

    #[test]
    fn a_line_above_an_address_is_an_addressee_only_when_it_reads_as_a_name() {
        assert!(texts(
            Kind::Person,
            "Facture payable dès réception\n12 rue des Lilas\n75011 Paris"
        )
        .is_empty());
        assert!(texts(Kind::Person, "Siège social\n\n12 rue des Lilas").is_empty());
    }

    #[test]
    fn a_form_filled_by_hand_gives_the_values_of_its_name_fields() {
        let form = "Nom de naissance : TRAN\nPrénom(s) : Kim Anh\nNé(e) le : 14/05/1988 à: Rennes\nNom et prénom : Marc Simon\nNom du service : accueil";
        assert_eq!(
            texts(Kind::Person, form),
            vec!["TRAN", "Kim", "Anh", "Marc Simon"]
        );
        assert_eq!(texts(Kind::BirthDate, form), vec!["14/05/1988"]);
        assert!(texts(Kind::BirthPlace, form).iter().all(|p| p != "TRAN"));
    }

    #[test]
    fn a_fine_notice_gives_its_addressee_and_not_its_headings() {
        let notice = "Date de l'avis de contravention\n\n15/03/2024\n\nMOREL BASILE\n\n18 RUE DES ACACIAS\n\n75017 PARIS\n\nDESCRIPTION DE L'INFRACTION\n\nEXCES DE VITESSE D'AU MOINS 5 KM/H PAR CONDUCTEUR DE VEHICULE A MOTEUR - VITESSE MAXIMALE AUTORISEE INFERIEURE OU EGALE A 50 KM/H\n\nNOTICE DE PAIEMENT\n\n* Sexe (M/F) *Date de naissance *à\n\n*Adresse";
        assert_eq!(texts(Kind::Person, notice), vec!["MOREL BASILE"]);
        assert!(texts(Kind::BirthPlace, notice).is_empty());
    }

    #[test]
    fn the_undersigned_names_the_witness() {
        assert_eq!(
            texts(
                Kind::Person,
                "Je soussignée Clémence Moreau atteste avoir vu"
            ),
            vec!["Clémence Moreau", "Moreau"]
        );
    }

    #[test]
    fn a_particle_stays_inside_the_name_it_belongs_to() {
        let found = of(Kind::Person, "Maître Anne de La Fontaine plaide");
        assert_eq!(found[0].text, "Anne de La Fontaine");
    }

    #[test]
    fn a_surname_in_capitals_is_proposed_and_the_court_is_not() {
        let found = of(
            Kind::Person,
            "COUR D'APPEL DE PARIS, arrêt rendu contre MARTIN BERNARD",
        );
        let texts: Vec<&str> = found.iter().map(|s| s.text.as_str()).collect();
        assert!(texts.contains(&"MARTIN BERNARD"), "{texts:?}");
        assert!(!texts.iter().any(|t| t.contains("APPEL")), "{texts:?}");
    }

    #[test]
    fn a_role_followed_by_a_colon_does_not_join_what_comes_after() {
        let found = of(Kind::Person, "APPELANT : LE PREFET, puis DUPONT MARTIN");
        let texts: Vec<&str> = found.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["DUPONT MARTIN"], "{texts:?}");
    }

    #[test]
    fn the_same_name_twice_is_one_suggestion_counted_twice() {
        let found = of(Kind::Person, "Monsieur Dupont puis monsieur Dupont encore");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].count, 2);
    }

    #[test]
    fn an_address_runs_from_its_number_to_its_city() {
        let found = of(Kind::Address, "demeurant 12 rue des Lilas, 75011 Paris, et");
        assert_eq!(found[0].text, "12 rue des Lilas, 75011 Paris");
    }

    #[test]
    fn an_address_stops_before_the_next_party() {
        let text = "demeurant 12 rue des Lilas\net Madame Claire Vassort, representee";
        assert_eq!(of(Kind::Address, text)[0].text, "12 rue des Lilas");
        assert_eq!(of(Kind::Person, text)[0].text, "Claire Vassort");
        assert_eq!(
            of(Kind::Address, "au 4 rue Madame, 75006 Paris")[0].text,
            "4 rue Madame, 75006 Paris"
        );
    }

    #[test]
    fn a_date_of_birth_is_taken_in_both_forms() {
        assert_eq!(
            of(Kind::BirthDate, "né le 12/03/1970 à Lyon")[0].text,
            "12/03/1970"
        );
        assert_eq!(
            of(Kind::BirthDate, "née le 3 mars 1970 à Lyon")[0].text,
            "3 mars 1970"
        );
    }

    #[test]
    fn an_email_stops_at_the_punctuation_that_follows_it() {
        let found = of(Kind::Email, "écrire à jean.dupont@exemple.fr, ou non");
        assert_eq!(found[0].text, "jean.dupont@exemple.fr");
    }

    #[test]
    fn a_phone_number_is_told_apart_from_a_docket_number() {
        assert_eq!(
            of(Kind::Phone, "joindre au 01 02 03 04 05 svp")[0].text,
            "01 02 03 04 05"
        );
        assert_eq!(
            of(Kind::Docket, "affaire n° 21/45678 du rôle")[0].text,
            "21/45678"
        );
    }

    #[test]
    fn an_identifier_is_proposed_only_when_its_check_digit_holds() {
        assert_eq!(
            texts(
                Kind::Identifier,
                "SIREN 732 829 320, SIRET 732 829 320 00074"
            ),
            vec!["732 829 320", "732 829 320 00074"]
        );
        assert!(texts(Kind::Identifier, "somme de 123 456 780 euros").is_empty());
        assert_eq!(
            texts(Kind::Identifier, "NIR 2 55 08 14 168 025 38"),
            vec!["2 55 08 14 168 025 38"]
        );
        assert!(texts(Kind::Identifier, "NIR 2 55 08 14 168 025 39").is_empty());
    }

    #[test]
    fn an_iban_stops_where_its_check_stops_holding() {
        assert_eq!(
            texts(
                Kind::Identifier,
                "IBAN FR76 3000 6000 0112 3456 7890 189 AU NOM"
            ),
            vec!["FR76 3000 6000 0112 3456 7890 189"]
        );
        assert_eq!(
            texts(Kind::Identifier, "compte FR7630006000011234567890189."),
            vec!["FR7630006000011234567890189"]
        );
        assert!(texts(Kind::Identifier, "FR76 3000 6000 0112 3456 7890 188").is_empty());
    }

    fn texts(kind: Kind, text: &str) -> Vec<String> {
        of(kind, text).into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn an_abbreviated_civility_carries_the_name_after_its_full_stop() {
        assert_eq!(
            texts(Kind::Person, "M. Anselme B..., requérant, et M.Tessandier"),
            vec!["Anselme", "Tessandier"]
        );
        assert_eq!(
            texts(Kind::Person, "M. Gaston Lebrouc, rapporteur public"),
            vec!["Gaston Lebrouc", "Lebrouc"]
        );
        assert_eq!(
            texts(Kind::Person, "Mme A. Pereira et M. J.-C. Vautrel"),
            vec!["Pereira", "Vautrel"]
        );
        assert_eq!(
            texts(Kind::Person, "Mme\nLina Quéverdo"),
            vec!["Lina Quéverdo", "Quéverdo"]
        );
    }

    #[test]
    fn a_name_after_a_civility_stops_before_the_procedure() {
        assert_eq!(
            texts(
                Kind::Person,
                "Me Nina LORVAL Avocat ; Mme Arvenel de la délivrance"
            ),
            vec!["Nina LORVAL", "LORVAL", "Arvenel"]
        );
        assert_eq!(texts(Kind::Person, "Mme de Vauclair"), vec!["de Vauclair"]);
        assert!(texts(Kind::Person, "M. Le Président du Conseil").is_empty());
        assert!(texts(Kind::Person, "Maître des Requêtes").is_empty());
    }

    #[test]
    fn a_plural_civility_or_a_firm_carries_its_whole_list() {
        assert_eq!(
            texts(
                Kind::Person,
                "MM. Durel, Fabre, Mmes Lestrade et Morin-Vasse, conseillers"
            ),
            vec!["Durel", "Fabre", "Lestrade", "Morin-Vasse"]
        );
        assert_eq!(
            texts(Kind::Person, "la SCP Rouvier, Maillard et Brovain"),
            vec!["Rouvier", "Maillard", "Brovain"]
        );
    }

    #[test]
    fn a_first_name_beside_a_surname_in_capitals_joins_it() {
        assert_eq!(
            texts(
                Kind::Person,
                "Nous, Clara VERNET, juge ; entendu BORISOV Igor"
            ),
            vec!["Clara VERNET", "VERNET", "BORISOV Igor", "BORISOV"]
        );
    }

    #[test]
    fn a_first_name_kept_beside_an_initial_is_proposed() {
        assert_eq!(
            texts(
                Kind::Person,
                "le requérant Younes K. soutient que Samia X... a"
            ),
            vec!["Younes", "Samia"]
        );
        assert!(texts(Kind::Person, "l'annexe B et l'article L. 761-1").is_empty());
    }

    #[test]
    fn a_relative_named_by_first_name_is_proposed() {
        assert_eq!(
            texts(
                Kind::Person,
                "ses petits-enfants Rayane, Selima et Omarou vivent"
            ),
            vec!["Rayane", "Selima", "Omarou"]
        );
        assert_eq!(
            texts(Kind::Person, "son fils Kévan est scolarisé"),
            vec!["Kévan"]
        );
    }

    #[test]
    fn a_birth_gives_its_date_and_its_town() {
        let text = "né le 1er juin 1985 à Tessalit (Mali), et sa sœur née à Kolvéra le 3 mars 1990";
        assert_eq!(
            texts(Kind::BirthDate, text),
            vec!["1er juin 1985", "3 mars 1990"]
        );
        assert_eq!(texts(Kind::BirthPlace, text), vec!["Tessalit", "Kolvéra"]);
        assert_eq!(
            texts(Kind::BirthDate, "née en France le 4 mai 2008"),
            vec!["4 mai 2008"]
        );
    }

    #[test]
    fn an_address_takes_a_comma_after_its_number_and_stops_on_its_postcode() {
        assert_eq!(
            texts(
                Kind::Address,
                "demeurant 14, rue des Tanneurs à Lyon (69003), par"
            ),
            vec!["14, rue des Tanneurs à Lyon (69003)"]
        );
        assert_eq!(
            texts(
                Kind::Address,
                "au 8 rue Vauban, appartement 12, 75011 Paris ; M. Dubreuil"
            ),
            vec!["8 rue Vauban, appartement 12, 75011 Paris"]
        );
        assert_eq!(
            texts(Kind::Address, "au 2 impasse Ferlinon ; Mme Bonal"),
            vec!["2 impasse Ferlinon"]
        );
    }

    #[test]
    fn headings_acronyms_and_places_in_capitals_are_not_names() {
        let text = "PAR CES MOTIFS DECIDE CASSE ET ANNULE ; l'OFPRA et le CESEDA ; \
                    LE PREFET DE LA HAUTE-VIENNE ; né au GHANA ; SEINE SAINT DENIS ; \
                    EXPOSÉ DE LA SITUATION ; THE FACTS ; FOR THESE REASONS";
        assert!(
            texts(Kind::Person, text).is_empty(),
            "{:?}",
            texts(Kind::Person, text)
        );
        assert_eq!(
            texts(
                Kind::Person,
                "Avocat(s) : SCP DURAMONT ET VESSIER  RÉPUBLIQUE"
            ),
            vec!["DURAMONT", "VESSIER"]
        );
    }

    #[test]
    fn a_cited_text_or_a_date_is_not_a_docket() {
        let text = "la directive 2008/115/CE, le règlement (UE) n° 604/2013, \
                    le 28/11/2016, l'année 2014/2015, et la requête n° 12/34567";
        assert_eq!(texts(Kind::Docket, text), vec!["12/34567"]);
    }

    #[test]
    fn a_document_without_anything_to_hide_proposes_nothing() {
        assert!(suggest(&source("le juge a statué sur le fond du litige")).is_empty());
    }

    #[test]
    fn a_birth_year_alone_or_after_a_label_is_a_birth_date() {
        assert_eq!(
            texts(Kind::BirthDate, "M. Paul Durand, né en 1996, demeurant"),
            vec!["1996"]
        );
        assert_eq!(
            texts(Kind::BirthDate, "né le [date] 12 mars 1970 à Lyon"),
            vec!["12 mars 1970"]
        );
        assert!(texts(Kind::BirthDate, "il ne peut en 1996 prétendre").is_empty());
    }

    #[test]
    fn a_residence_carries_the_address_up_to_its_town() {
        assert_eq!(
            texts(
                Kind::Address,
                "Mme Durand, demeurant 12 rue des Lilas 13015 Marseille, appelante"
            ),
            vec!["12 rue des Lilas 13015 Marseille"]
        );
    }

    #[test]
    fn fifteen_digits_with_a_wrong_key_are_a_nir_only_after_its_context() {
        let digits = "185031305512300";
        assert!(texts(Kind::Identifier, &format!("le montant {digits} est dû")).is_empty());
        assert_eq!(
            texts(
                Kind::Identifier,
                &format!("immatriculé à la sécurité sociale sous le n° {digits}")
            ),
            vec![digits]
        );
    }

    #[test]
    fn a_registration_plate_is_an_identifier() {
        assert_eq!(
            texts(Kind::Identifier, "le véhicule AB-123-CD a été saisi"),
            vec!["AB-123-CD"]
        );
        assert_eq!(
            texts(Kind::Identifier, "un scooter immatriculé 1234XY56 volé"),
            vec!["1234XY56"]
        );
    }

    #[test]
    fn a_kin_word_reaches_past_its_qualifier_to_the_name() {
        assert_eq!(
            texts(Kind::Person, "son fils mineur Kevin a déclaré"),
            vec!["Kevin"]
        );
    }

    #[test]
    fn an_illegible_run_of_capitals_is_not_a_name() {
        assert!(texts(Kind::Person, "le tampon MRRKT XQZZ figure en marge").is_empty());
    }
}
