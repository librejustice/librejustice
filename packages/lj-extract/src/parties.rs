//! A decision's actors: from the 7 flat NER cells, builds the canonical rows,
//! value, quality, side, `resolve_key`, `nature`, evidence spans
//! (`char_starts`/`char_ends`, codepoints of `full_text`) and the mentions the
//! scan read.
//!
//! Les spans se dérivent par matching replié déterministe de la valeur dans le
//! texte (`lj_fold`, runs d'espaces équivalents). Une valeur de provenance
//! métadonnée sans occurrence corps a des spans vides. Le pliage et la clé de
//! résolution sont calculés ICI, le SQL ne replie jamais.
//!
//! Pour `counsel_name`, la clé s'enrichit des évidences d'apposition : prénom
//! devant l'occurrence → clé nom complet, barreau après l'occurrence → colonne
//! `barreau` (slug officiel CNB). Le rôle explicite se capte au même endroit et
//! les côtés contradictoires fusionnent en côté indéterminé.

use lj_fold::fold_chars;

use crate::registry::{PartyRegistry, Quality, Side};

/// Sigles de forme juridique dépouillés en tête de clé (les dénominations
/// SIRENE ne portent pas la forme — colonne `forme` séparée).
const FORM_SIGLES: &[&str] = &[
    "sa", "sas", "sasu", "sarl", "eurl", "snc", "sci", "scp", "scm", "scea", "gaec", "earl", "gie",
    "sem", "seml", "selarl", "selarlu", "seleurl", "selas", "selasu", "selafa", "selca", "sccv",
    "scop", "spfpl", "aarpi", "sasp", "sepa",
];

/// Qualificatifs consommés après « société »/« ste » (formes développées :
/// « société par actions simplifiée X » → « x »).
const SOC_QUALIFS: &[&str] = &[
    "anonyme",
    "civile",
    "immobiliere",
    "cooperative",
    "par",
    "actions",
    "action",
    "simplifiee",
    "unipersonnelle",
    "a",
    "responsabilite",
    "limitee",
    "en",
    "nom",
    "collectif",
    "commandite",
    "simple",
    "exercice",
    "liberal",
    "liberale",
    "professionnelle",
    "d'exercice",
];

/// Past it, a value is a sentence or a run of names; and Postgres caps the size
/// of a GIN entry, which would fail the write.
const MAX_RESOLVE_KEY_BYTES: usize = 250;

/// Clé de résolution : pliage canonique, tête dépouillée (article, sigle de
/// forme, « société » et ses qualificatifs). Vide après dépouille → repli
/// sur le pliage entier (une valeur qui n'est QU'une forme reste elle-même).
pub fn resolve_key(value: &str) -> String {
    let c = lj_fold::fold(value);
    let mut s = c.as_str();
    for art in ["la ", "le ", "les ", "l'"] {
        if let Some(r) = s.strip_prefix(art) {
            s = r;
            break;
        }
    }
    let toks: Vec<&str> = s.split(' ').collect();
    let mut i = 0;
    if let Some(t) = toks.first() {
        let bare = t.replace('.', "");
        if FORM_SIGLES.contains(&bare.as_str()) {
            i = 1;
        } else if matches!(bare.as_str(), "societe" | "ste") {
            i = 1;
            while i < toks.len() && SOC_QUALIFS.contains(&toks[i].replace('.', "").as_str()) {
                i += 1;
            }
        }
    }
    let rest = toks[i..].join(" ");
    if rest.is_empty() {
        c
    } else {
        rest
    }
}

/// Slugs officiels des barreaux du registre CNB — segment barreau des uid
/// `cnb:<barreau>:<nom-prenom>`. Table de normalisation embarquée (même statut
/// que `FORM_SIGLES`) ; à rafraîchir si un rechargement CNB fait apparaître un
/// barreau.
const BARREAU_SLUGS: &[&str] = &[
    "agen",
    "ain",
    "aix-en-provence",
    "ajaccio",
    "albertville",
    "albi",
    "alencon",
    "ales",
    "alpes-de-haute-provence",
    "amiens",
    "angers",
    "annecy",
    "ardeche",
    "ardennes",
    "argentan",
    "ariege",
    "arras",
    "aube",
    "aurillac",
    "auxerre",
    "avesnes-sur-helpe",
    "aveyron",
    "avignon",
    "bastia",
    "bayonne",
    "beauvais",
    "belfort",
    "bergerac-sarlat",
    "besancon",
    "bethune",
    "beziers",
    "blois",
    "bonneville-et-les-pays-du-montblanc",
    "bordeaux",
    "boulogne-sur-mer",
    "bourges",
    "bourgoin-jallieu",
    "brest",
    "briey",
    "brive",
    "caen",
    "cambrai",
    "carcassonne",
    "carpentras",
    "castres",
    "chalon-sur-saone",
    "chalons-en-champagne",
    "chambery",
    "charente",
    "chartres",
    "chateauroux",
    "cherbourg",
    "clermont-ferrand",
    "colmar",
    "compiegne",
    "coutances-avranches",
    "creuse",
    "cusset-vichy",
    "dax",
    "deux-sevres",
    "dieppe",
    "dijon",
    "douai",
    "draguignan",
    "dunkerque",
    "epinal",
    "essonne",
    "eure",
    "fontainebleau",
    "fort-de-france-(martinique)",
    "gers",
    "grasse",
    "grenoble",
    "guadeloupe,-saint-martin,-saint-barthelemy",
    "guyane",
    "haute-loire",
    "haute-marne",
    "haute-saone",
    "hautes-alpes",
    "hauts-de-seine",
    "jura",
    "la-roche-sur-yon",
    "la-rochelle-rochefort",
    "laon",
    "laval",
    "le-havre",
    "le-mans",
    "les-sables-d-olonne",
    "libourne",
    "lille",
    "limoges",
    "lisieux",
    "lorient",
    "lot",
    "lozere",
    "lyon",
    "macon",
    "marseille",
    "mayotte",
    "meaux",
    "melun",
    "metz",
    "meuse",
    "mont-de-marsan",
    "montargis",
    "montbeliard",
    "montlucon",
    "montpellier",
    "moulins",
    "mulhouse",
    "nancy",
    "nantes",
    "narbonne",
    "nevers",
    "nice",
    "nimes",
    "noumea-(nouvelle-caledonie)",
    "orleans",
    "papeete---tahiti-(nouvelle-caledonie)",
    "paris",
    "pau",
    "perigueux",
    "poitiers",
    "pyrenees-orientales",
    "quimper",
    "reims",
    "rennes",
    "roanne",
    "rouen",
    "saint-brieuc",
    "saint-denis-de-la-reunion",
    "saint-etienne",
    "saint-gaudens",
    "saint-malo-dinan",
    "saint-nazaire",
    "saint-omer",
    "saint-pierre-de-la-reunion",
    "saint-quentin",
    "saintes",
    "sarreguemines",
    "saumur",
    "saverne",
    "seine-saint-denis",
    "senlis",
    "sens",
    "soissons",
    "strasbourg",
    "tarascon",
    "tarbes",
    "tarn-et-garonne",
    "thionville",
    "thonon-les-bains,-leman-et-genevois",
    "toulon",
    "toulouse",
    "tours",
    "tulle",
    "val-d-oise",
    "val-de-marne",
    "valence",
    "valenciennes",
    "vannes",
    "versailles",
    "vienne",
    "villefranche-sur-saone",
];

/// Ville-siège d'usage courant dans les décisions → slug du barreau
/// (départemental ou composé) correspondant. Les barreaux ultramarins n'y sont
/// pas : leur graphie se dérive de `lj_core::outremer`
/// ([`barreau_outremer`]), table partagée avec le correcteur de
/// `jurisdiction_code`.
const BARREAU_ALIAS: &[(&str, &str)] = &[
    ("avesnes", "avesnes-sur-helpe"),
    ("avranches", "coutances-avranches"),
    ("angouleme", "charente"),
    ("auch", "gers"),
    ("bar-le-duc", "meuse"),
    ("bergerac", "bergerac-sarlat"),
    ("bobigny", "seine-saint-denis"),
    ("bonneville", "bonneville-et-les-pays-du-montblanc"),
    ("bourg-en-bresse", "ain"),
    ("cahors", "lot"),
    ("cergy-pontoise", "val-d-oise"),
    ("charleville-mezieres", "ardennes"),
    ("chaumont", "haute-marne"),
    ("coutances", "coutances-avranches"),
    ("creteil", "val-de-marne"),
    ("cusset", "cusset-vichy"),
    ("digne", "alpes-de-haute-provence"),
    ("digne-les-bains", "alpes-de-haute-provence"),
    ("dinan", "saint-malo-dinan"),
    ("evreux", "eure"),
    ("evry", "essonne"),
    ("evry-courcouronnes", "essonne"),
    ("foix", "ariege"),
    ("gap", "hautes-alpes"),
    ("gueret", "creuse"),
    ("la-rochelle", "la-rochelle-rochefort"),
    ("le-puy", "haute-loire"),
    ("le-puy-en-velay", "haute-loire"),
    ("lons-le-saunier", "jura"),
    ("mende", "lozere"),
    ("montauban", "tarn-et-garonne"),
    ("nanterre", "hauts-de-seine"),
    ("niort", "deux-sevres"),
    ("perpignan", "pyrenees-orientales"),
    ("pontoise", "val-d-oise"),
    ("privas", "ardeche"),
    ("rochefort", "la-rochelle-rochefort"),
    ("rodez", "aveyron"),
    ("sables-d-olonne", "les-sables-d-olonne"),
    ("saint-malo", "saint-malo-dinan"),
    ("sarlat", "bergerac-sarlat"),
    ("thonon", "thonon-les-bains,-leman-et-genevois"),
    ("thonon-les-bains", "thonon-les-bains,-leman-et-genevois"),
    ("troyes", "aube"),
    ("vesoul", "haute-saone"),
    ("vichy", "cusset-vichy"),
];

/// Têtes de personne morale PUBLIQUE (préfixe sur clé canonique sans
/// article). Cœur conservateur : collectivités, établissements publics
/// nommés comme tels, État et ses organes.
const PUBLIC_HEADS: &[&str] = &[
    "etat",
    "commune de",
    "commune d'",
    "communes de",
    "ville de",
    "ville d'",
    "departement",
    "region ",
    "collectivite",
    "metropole",
    "communaute de communes",
    "communaute d'agglomeration",
    "communaute urbaine",
    "syndicat mixte",
    "etablissement public",
    "office public",
    "office national",
    "office francais",
    "centre hospitalier",
    "chu ",
    "chr ",
    "assistance publique",
    "centre communal d'action sociale",
    "caisse des ecoles",
    "caisse des depots",
    "service departemental d'incendie",
    "ministre",
    "ministere",
    "premier ministre",
    "garde des sceaux",
    "prefet",
    "prefecture",
    "rectorat",
    "universite",
    "agence regionale de sante",
    "pole emploi",
    "france travail",
];

/// Têtes de personne morale PRIVÉE (au-delà des formes juridiques déjà
/// couvertes par `FORM_SIGLES` / « société »).
const PRIVATE_HEADS: &[&str] = &[
    "association",
    "syndicat des coproprietaires",
    "syndicat de coproprietaires",
    "syndicat secondaire",
    "mutuelle",
    "fondation",
    "banque",
    "caisse",
    "credit ",
    "compagnie",
    "cabinet",
    "clinique",
    "polyclinique",
    "groupement",
    "cooperative",
    "federation",
    "comite d'entreprise",
    "comite social et economique",
];

/// Nature de l'acteur (axe 1 fermé). `None` = le moteur ne se prononce pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nature {
    Physique,
    MoralePrivee,
    MoralePublique,
}

impl Nature {
    pub fn as_str(self) -> &'static str {
        match self {
            Nature::Physique => "physique",
            Nature::MoralePrivee => "morale_privee",
            Nature::MoralePublique => "morale_publique",
        }
    }
}

/// Classe la nature d'une valeur émise dans une cellule de qualité donnée.
/// `counsel_name` est structurellement une personne physique (l'avocat) ;
/// `law_firm` une structure d'exercice (morale privée) ; les parties /
/// intervenants se classent par la tête de dénomination — le doute rend
/// `None`.
pub fn nature(quality: &str, value: &str) -> Option<Nature> {
    match quality {
        "counsel_name" => return Some(Nature::Physique),
        "law_firm" => return Some(Nature::MoralePrivee),
        _ => {}
    }
    let c = lj_fold::fold(value);
    let mut s = c.as_str();
    for art in ["la ", "le ", "les ", "l'"] {
        if let Some(r) = s.strip_prefix(art) {
            s = r;
            break;
        }
    }
    if matches!(s, "etat" | "etat francais") {
        return Some(Nature::MoralePublique);
    }
    if PUBLIC_HEADS
        .iter()
        .any(|h| s.starts_with(h) && (h.ends_with(' ') || h.ends_with('\'') || word_bounded(s, h)))
    {
        return Some(Nature::MoralePublique);
    }
    let first = s.split(' ').next().unwrap_or("");
    let bare = first.replace('.', "");
    if FORM_SIGLES.contains(&bare.as_str()) || matches!(bare.as_str(), "societe" | "ste") {
        return Some(Nature::MoralePrivee);
    }
    if PRIVATE_HEADS
        .iter()
        .any(|h| s.starts_with(h) && (h.ends_with(' ') || word_bounded(s, h)))
    {
        return Some(Nature::MoralePrivee);
    }
    if matches!(bare.as_str(), "m" | "mme" | "mlle" | "monsieur" | "madame") {
        return Some(Nature::Physique);
    }
    None
}

/// Le préfixe `h` de `s` tombe sur une frontière de mot (fin de chaîne ou
/// espace suivant).
fn word_bounded(s: &str, h: &str) -> bool {
    s.len() == h.len() || s[h.len()..].starts_with(' ')
}

/// Occurrences d'une valeur dans le texte plié : spans `[start, end)` en
/// chars pliés. Un espace de la valeur apparie un run d'espaces du texte
/// (les valeurs de métadonnée sont à blancs simples, le texte non).
/// Non-chevauchant glouton gauche→droite.
pub fn evidence_spans(folded: &[char], value: &str) -> (Vec<i32>, Vec<i32>) {
    let needle: Vec<char> = fold_chars(value).chars().collect();
    let needle: &[char] = needle
        .iter()
        .position(|c| *c != ' ')
        .map(|s| {
            let e = needle.iter().rposition(|c| *c != ' ').unwrap() + 1;
            &needle[s..e]
        })
        .unwrap_or(&[]);
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    if needle.is_empty() {
        return (starts, ends);
    }
    let first = needle[0];
    let mut i = 0usize;
    while i < folded.len() {
        if folded[i] != first {
            i += 1;
            continue;
        }
        if let Some(end) = match_at(folded, i, needle) {
            starts.push(i as i32);
            ends.push(end as i32);
            i = end;
        } else {
            i += 1;
        }
    }
    (starts, ends)
}

/// Apparie `needle` dans `folded` à partir de `at`, un espace du needle
/// consommant un run d'espaces du texte. Renvoie l'index de fin (exclu).
fn match_at(folded: &[char], at: usize, needle: &[char]) -> Option<usize> {
    let mut i = at;
    let mut j = 0usize;
    while j < needle.len() {
        if needle[j] == ' ' {
            if i >= folded.len() || folded[i] != ' ' {
                return None;
            }
            while i < folded.len() && folded[i] == ' ' {
                i += 1;
            }
            j += 1;
        } else {
            if i >= folded.len() || folded[i] != needle[j] {
                return None;
            }
            i += 1;
            j += 1;
        }
    }
    Some(i)
}

/// Évidences de résolution d'un avocat : clé nom complet quand un prénom est
/// trouvé en apposition devant une occurrence (« Me Laura JAVERT » pour la
/// valeur `Javert`), slug du barreau en apposition après, rôle explicite. Une
/// valeur nom-seul sans prénom garde sa clé nom-seul — structurellement
/// irrésoluble contre les dénominations complètes du registre.
fn counsel_evidence(
    folded: &[char],
    value: &str,
    starts: &[i32],
    ends: &[i32],
) -> (String, Option<String>, Option<&'static str>) {
    let c = lj_fold::fold(value);
    let key = if c.contains(' ') {
        c
    } else {
        starts
            .iter()
            .find_map(|s| given_before(folded, *s as usize))
            .map(|g| format!("{g} {c}"))
            .unwrap_or(c)
    };
    let barreau = ends.iter().find_map(|e| barreau_after(folded, *e as usize));
    let role = starts
        .iter()
        .zip(ends)
        .find_map(|(s, e)| counsel_role(folded, *s as usize, *e as usize));
    (key, barreau, role)
}

/// Rôle explicite en apposition. La substitution encadre le nom : « Me X,
/// substituant Me Y » fait de X le `substituant` (présent à l'audience) et de Y
/// le `substitue` (titulaire du dossier) ; « substitué par Me Z » inverse la
/// lecture. `postulant`/`plaidant` suivent le nom (style CA). La fenêtre avant
/// s'arrête au prochain « Me »/« Maître » pour ne jamais lire le marqueur de
/// l'avocat voisin.
fn counsel_role(folded: &[char], start: usize, end: usize) -> Option<&'static str> {
    fn bare(t: &str) -> &str {
        t.trim_matches(|c: char| !c.is_alphanumeric())
    }
    // — avant l'occurrence : le nom est l'objet du marqueur —
    let from = start.saturating_sub(48);
    let w: String = folded[from..start].iter().collect();
    let toks: Vec<&str> = w.split_whitespace().map(bare).collect();
    if let Some(at) = toks.iter().rposition(|t| matches!(*t, "me" | "maitre")) {
        let head = &toks[..at];
        if head.last() == Some(&"substituant") {
            return Some("substitue");
        }
        if head.len() >= 2
            && head[head.len() - 1] == "par"
            && head[head.len() - 2].starts_with("substitue")
        {
            return Some("substituant");
        }
    }
    // — après l'occurrence : le nom est le sujet du marqueur —
    let to = (end + 64).min(folded.len());
    let w: String = folded[end..to].iter().collect();
    let mut prev = "";
    for t in w.split_whitespace().map(bare) {
        if matches!(t, "me" | "maitre") {
            break; // l'apposition de l'avocat suivant commence
        }
        if t == "substituant" {
            return Some("substituant");
        }
        if t == "par" && prev.starts_with("substitue") {
            return Some("substitue");
        }
        if t.starts_with("postulant") {
            return Some("postulant");
        }
        if t.starts_with("plaidant") {
            return Some("plaidant");
        }
        prev = t;
    }
    None
}

/// Fenêtre d'apposition cabinet après une occurrence de counsel : « Me X **de
/// la** SELARL Y ». S'ouvre sur les connecteurs que l'extraction utilise pour
/// ébarber la queue (`trim_counsel_tail`), collés au nom, et se ferme au
/// premier marqueur `me`/`maitre` — « Me X de la SCP Y, Me Z » ne rattache pas
/// Z au cabinet. Renvoie `[début, fin)` de la zone où le span d'un `law_firm`
/// vaut apposition.
/// Jeu toléré entre la fin du nom de la partie et le connecteur : la virgule,
/// et un qualificatif court que le trim de valeur a laissé dehors.
const PARTY_GAP: usize = 12;

/// Connecteur de représentation devant le conseil (« <partie>, représentée par
/// Me X ») : rend la position où le nom de la partie doit finir, juste avant le
/// connecteur. La fenêtre porte à 72 caractères, un cabinet pouvant s'intercaler.
fn party_window(folded: &[char], start: usize) -> Option<usize> {
    const CONN: [&str; 11] = [
        "represente par",
        "representee par",
        "representes par",
        "representees par",
        "assiste de",
        "assistee de",
        "assiste par",
        "assistee par",
        "ayant pour avocat",
        "comparant par",
        "rep/assistant",
    ];
    let from = start.saturating_sub(72);
    let w: String = folded[from..start].iter().collect();
    let at = CONN.iter().filter_map(|c| w.rfind(c)).max()?;
    Some(from + w[..at].chars().count())
}

fn firm_window(folded: &[char], end: usize) -> Option<(usize, usize)> {
    const LIENS: [&str; 6] = [" de la ", " de l'", " du ", " de ", " - ", " – "];
    let to = (end + 64).min(folded.len());
    let w: String = folded[end..to].iter().collect();
    let lien = LIENS.iter().find(|l| w.starts_with(**l))?;
    let from = end + lien.chars().count();
    // Le nom du cabinet peut être précédé d'une forme sociale redondante
    // (« de la SELARL SELARL LE GUILLOU RODRIGUES ») : la fenêtre couvre le reste,
    // bornée par le marqueur de l'avocat voisin.
    let mut close = to;
    let mut i = from;
    while i < to {
        let tok = i;
        while i < to && !folded[i].is_whitespace() {
            i += 1;
        }
        let bare: String = folded[tok..i].iter().collect();
        if matches!(
            bare.trim_matches(|c: char| !c.is_alphanumeric()),
            "me" | "maitre"
        ) {
            close = tok;
            break;
        }
        while i < to && folded[i].is_whitespace() {
            i += 1;
        }
    }
    (from < close).then_some((from, close))
}

/// Prénom en apposition : le texte entre le dernier marqueur « me »/« maitre »
/// de la fenêtre arrière et l'occurrence — 1-2 tokens de lettres.
fn given_before(folded: &[char], start: usize) -> Option<String> {
    if start == 0 || !folded[start - 1].is_whitespace() {
        return None;
    }
    let from = start.saturating_sub(48);
    let w: String = folded[from..start].iter().collect();
    let toks: Vec<&str> = w.split_whitespace().collect();
    let at = toks.iter().rposition(|t| matches!(*t, "me" | "maitre"))?;
    // Marqueur en tête de fenêtre : fiable seulement sur frontière réelle.
    if at == 0 && from > 0 && !folded[from - 1].is_whitespace() {
        return None;
    }
    let given = &toks[at + 1..];
    if given.is_empty()
        || given.len() > 2
        || given.iter().any(|t| {
            t.chars().count() < 2
                || !t
                    .chars()
                    .all(|c| c.is_alphabetic() || c == '\'' || c == '-')
                || matches!(*t, "le" | "la" | "les" | "de" | "du" | "des")
        })
    {
        return None;
    }
    Some(given.join(" "))
}

/// Slug du barreau en apposition (« avocat au barreau de X ») après une
/// occurrence : plus-long-préfixe contre les slugs officiels + alias.
fn barreau_after(folded: &[char], end: usize) -> Option<String> {
    let to = (end + 96).min(folded.len());
    let w: String = folded[end..to].iter().collect();
    let p = w.find("barreau")?;
    if w[..p]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphabetic())
    {
        return None;
    }
    let mut s = slugify(&w[p + "barreau".len()..]);
    for pref in ["de-la-", "de-l-", "de-", "du-", "des-", "d-"] {
        if let Some(r) = s.strip_prefix(pref) {
            s = r.to_string();
            break;
        }
    }
    let mut best: Option<&str> = None;
    for k in BARREAU_SLUGS
        .iter()
        .copied()
        .chain(BARREAU_ALIAS.iter().map(|(a, _)| *a))
        .chain(lj_core::outremer::graphies())
    {
        let bounded = s == k || s.strip_prefix(k).is_some_and(|r| r.starts_with('-'));
        if bounded && best.is_none_or(|b| k.len() > b.len()) {
            best = Some(k);
        }
    }
    best.map(|k| {
        BARREAU_ALIAS
            .iter()
            .find(|(a, _)| *a == k)
            .map(|(_, c)| *c)
            .or_else(|| barreau_outremer(k))
            .unwrap_or(k)
            .to_string()
    })
}

/// Barreau ultramarin depuis n'importe quelle graphie du lieu : le registre
/// CNB nomme tantôt par ressort (« guyane »), tantôt par ville de siège
/// (« fort-de-france-(martinique) »), là où la décision écrit l'autre — on
/// essaie donc toute la classe d'équivalence ([`lj_core::outremer`]) contre
/// les slugs officiels, en tête de slug.
fn barreau_outremer(place: &str) -> Option<&'static str> {
    lj_core::outremer::equivalents(place).iter().find_map(|q| {
        BARREAU_SLUGS.iter().copied().find(|k| {
            let k = lj_fold::slug(k, '-');
            k == *q || k.strip_prefix(q).is_some_and(|r| r.starts_with('-'))
        })
    })
}

/// Slug : alphanumériques ASCII conservés, toute autre séquence → un tiret,
/// sans tiret de tête.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out
}

/// An actor of the decision, ready to persist.
#[derive(Debug, Clone)]
pub struct ActorRow {
    pub quality: &'static str,
    pub side: Option<&'static str>,
    pub value: String,
    pub resolve_key: String,
    pub nature: Option<Nature>,
    pub barreau: Option<String>,
    /// `substituant` | `substitue` | `postulant` | `plaidant`, `None` = aucun
    /// marqueur.
    pub role: Option<&'static str>,
    /// `ord` de la ligne `law_firm` apposée au conseil, `None` hors counsel ou sans
    /// apposition lisible.
    pub firm_ord: Option<i32>,
    /// `ord` de la ligne partie que ce conseil représente. C'est l'arête dont
    /// `side` est dérivé ; `None` quand aucun connecteur ne la nomme.
    pub party_ord: Option<i32>,
    /// `ord` de la ligne conseil substituée, posée sur le `substituant`.
    pub sub_ord: Option<i32>,
    pub char_starts: Vec<i32>,
    pub char_ends: Vec<i32>,
    /// The registry mentions this actor was read from, `[start, end, …]` in
    /// chars of `full_text`, sorted and disjoint. Empty for a metadata value.
    pub mentions: Vec<i32>,
}

/// Cellule NER plate ; l'ordre des cellules définit `ord` intra-décision.
/// `nature` la force quand la cellule la connaît : un pseudonyme n'a aucune tête
/// de dénomination que [`nature`] puisse classer.
#[derive(Debug, Clone, Copy)]
pub struct Cell<'a> {
    pub quality: &'static str,
    pub side: Option<&'static str>,
    pub nature: Option<Nature>,
    pub values: &'a [String],
}

impl<'a> Cell<'a> {
    pub fn new(quality: &'static str, side: Option<&'static str>, values: &'a [String]) -> Self {
        Self {
            quality,
            side,
            nature: None,
            values,
        }
    }

    pub fn physique(
        quality: &'static str,
        side: Option<&'static str>,
        values: &'a [String],
    ) -> Self {
        Self {
            quality,
            side,
            nature: Some(Nature::Physique),
            values,
        }
    }
}

/// Construit les lignes acteur d'une décision depuis ses cellules plates.
/// Le texte est plié UNE fois ; chaque valeur reçoit ses spans-évidences,
/// sa nature et sa clé de résolution.
pub fn actor_rows(full_text: &str, cells: &[Cell<'_>]) -> Vec<ActorRow> {
    let norm = crate::compiled::Norm::new(full_text);
    rows_on(&norm.folded, &norm.byte2char, cells, None)
}

/// Same, on the text the scan already folded (prod path): no second fold per
/// decision, and each actor gets the mentions the scan read it from.
pub fn actor_rows_scanned(scan: &crate::scan::DocScan, cells: &[Cell<'_>]) -> Vec<ActorRow> {
    rows_on(
        scan.folded(),
        scan.byte2char(),
        cells,
        Some(scan.party_registry()),
    )
}

/// The registry cell a flat cell projects.
fn registry_cell(cell: &Cell<'_>) -> Option<(Option<Side>, Quality)> {
    let side = match cell.side {
        Some("applicant") => Some(Side::Applicant),
        Some("defendant") => Some(Side::Defendant),
        _ => None,
    };
    let quality = match cell.quality {
        "party" if cell.nature == Some(Nature::Physique) => Quality::Individual,
        "party" => Quality::Party,
        "law_firm" => Quality::LawFirm,
        "counsel_name" => Quality::CounselName,
        "intervenor" => Quality::Intervenor,
        _ => return None,
    };
    Some((side, quality))
}

/// The registry mentions of a cell, one list per value. A mention goes to the
/// value its rendering folds to, else to the one value whose words contain its
/// own or are contained in them: the flat cell trims, merges and dedupes what
/// the registry read. A mention matching no value or several stays unattributed.
fn cell_mentions(registry: &PartyRegistry, cell: &Cell<'_>) -> Vec<Vec<i32>> {
    let mut out = vec![Vec::new(); cell.values.len()];
    let Some((side, quality)) = registry_cell(cell) else {
        return out;
    };
    let words = |v: &str| -> Vec<String> {
        lj_fold::slug(v, ' ')
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    let values: Vec<Vec<String>> = cell.values.iter().map(|v| words(v)).collect();
    for entity in registry.mentions(side, quality) {
        let read = if quality == Quality::CounselName {
            words(&crate::extract::trim_counsel_tail(entity.value.clone()))
        } else {
            words(&entity.value)
        };
        if read.is_empty() {
            continue;
        }
        let exact: Vec<usize> = (0..values.len()).filter(|&i| values[i] == read).collect();
        let target = match exact[..] {
            [i] => Some(i),
            [] => {
                let near: Vec<usize> = (0..values.len())
                    .filter(|&i| {
                        !values[i].is_empty()
                            && (read.iter().all(|w| values[i].contains(w))
                                || values[i].iter().all(|w| read.contains(w)))
                    })
                    .collect();
                match near[..] {
                    [i] => Some(i),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(i) = target {
            out[i].extend([entity.s as i32, entity.e as i32]);
        }
    }
    out
}

/// Flat `[start, end, …]` pairs sorted by start, overlapping pairs merged.
fn disjoint(mentions: Vec<i32>) -> Vec<i32> {
    let mut pairs: Vec<(i32, i32)> = mentions
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[s, e]| (*s, *e))
        .collect();
    pairs.sort_unstable();
    let mut out: Vec<i32> = Vec::with_capacity(mentions.len());
    for (s, e) in pairs {
        match out.last_mut() {
            Some(last) if s < *last => *last = (*last).max(e),
            _ => out.extend([s, e]),
        }
    }
    out
}

/// Les spans se calculent sur les chars pliés et sortent en chars de
/// `full_text`, un char source pouvant se plier en plusieurs.
fn rows_on(
    folded: &str,
    byte2char: &[usize],
    cells: &[Cell<'_>],
    registry: Option<&PartyRegistry>,
) -> Vec<ActorRow> {
    let source: Vec<usize> = folded
        .char_indices()
        .map(|(b, _)| byte2char[b])
        .chain(std::iter::once(byte2char[folded.len()]))
        .collect();
    let folded: Vec<char> = folded.chars().collect();
    let mut rows = Vec::new();
    for cell in cells {
        let mut read = registry
            .map(|r| cell_mentions(r, cell))
            .unwrap_or_default()
            .into_iter();
        for value in cell.values {
            let mentions = read.next().unwrap_or_default();
            let (char_starts, char_ends) = evidence_spans(&folded, value);
            let (key, barreau, role) = if cell.quality == "counsel_name" {
                counsel_evidence(&folded, value, &char_starts, &char_ends)
            } else {
                (resolve_key(value), None, None)
            };
            if key.len() > MAX_RESOLVE_KEY_BYTES {
                continue;
            }
            rows.push(ActorRow {
                quality: cell.quality,
                side: cell.side,
                value: value.clone(),
                resolve_key: key,
                nature: cell.nature.or_else(|| nature(cell.quality, value)),
                barreau,
                role,
                firm_ord: None,
                party_ord: None,
                sub_ord: None,
                char_starts,
                char_ends,
                mentions,
            });
        }
    }
    // Cohérence de côté : une même valeur counsel émise des deux côtés par le NER
    // est du bruit — un avocat ne représente jamais les deux parties. Une seule
    // ligne, côté indéterminé ; spans identiques par construction (même valeur
    // pliée), premier rôle non-nul conservé.
    let mut out: Vec<ActorRow> = Vec::with_capacity(rows.len());
    let mut counsel_at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in rows {
        if r.quality != "counsel_name" {
            out.push(r);
            continue;
        }
        match counsel_at.entry(lj_fold::fold(&r.value)) {
            std::collections::hash_map::Entry::Occupied(e) => {
                let prev = &mut out[*e.get()];
                if prev.side != r.side {
                    prev.side = None;
                }
                if prev.role.is_none() {
                    prev.role = r.role;
                }
                prev.mentions.extend(r.mentions);
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(out.len());
                out.push(r);
            }
        }
    }
    // Apposition cabinet : après la fusion de côté, donc sur les index définitifs
    // — `ord` EST l'index de la ligne chez les deux écrivains. Le cabinet retenu
    // est celui dont un span commence au plus tôt dans la fenêtre ouverte par le
    // connecteur.
    let firms: Vec<(usize, i32)> = out
        .iter()
        .enumerate()
        .filter(|(_, r)| r.quality == "law_firm")
        .flat_map(|(i, r)| r.char_starts.iter().map(move |s| (i, *s)))
        .collect();
    if !firms.is_empty() {
        for row in out.iter_mut().filter(|r| r.quality == "counsel_name") {
            row.firm_ord = row
                .char_ends
                .iter()
                .filter_map(|e| firm_window(&folded, *e as usize))
                .filter_map(|(from, close)| {
                    firms
                        .iter()
                        .filter(|(_, s)| (from..close).contains(&(*s as usize)))
                        .min_by_key(|(_, s)| *s)
                        .map(|(j, s)| (*s, *j as i32))
                })
                .min()
                .map(|(_, j)| j);
        }
    }
    // Arête de représentation : la partie dont un span finit juste avant le
    // connecteur qui ouvre ce conseil. `ord` EST l'index de la ligne.
    let parties: Vec<(usize, i32)> = out
        .iter()
        .enumerate()
        .filter(|(_, r)| r.quality == "party")
        .flat_map(|(i, r)| r.char_ends.iter().map(move |e| (i, *e)))
        .collect();
    if !parties.is_empty() {
        for row in out
            .iter_mut()
            .filter(|r| matches!(r.quality, "counsel_name" | "law_firm"))
        {
            row.party_ord = row
                .char_starts
                .iter()
                .filter_map(|s| party_window(&folded, *s as usize))
                .filter_map(|fin| {
                    parties
                        .iter()
                        .filter(|(_, e)| (*e as usize) <= fin && (*e as usize) + PARTY_GAP > fin)
                        .max_by_key(|(_, e)| *e)
                        .map(|(j, e)| (fin - *e as usize, *j as i32))
                })
                .min()
                .map(|(_, j)| j);
        }
    }
    // Substitution : le `substituant` pointe la ligne `substitue` la plus proche
    // dans le texte, le connecteur pouvant précéder ou suivre selon la tournure
    // (« Me X, substituant Me Y » / « Me X substitué par Me Z »).
    let substitues: Vec<(usize, i32)> = out
        .iter()
        .enumerate()
        .filter(|(_, r)| r.role == Some("substitue"))
        .flat_map(|(i, r)| r.char_starts.iter().map(move |s| (i, *s)))
        .collect();
    if !substitues.is_empty() {
        for row in out
            .iter_mut()
            .filter(|r| r.role == Some("substituant") && !r.char_starts.is_empty())
        {
            let ancre = row.char_starts[0];
            row.sub_ord = substitues
                .iter()
                .min_by_key(|(_, s)| (s - ancre).abs())
                .map(|(j, _)| *j as i32);
        }
    }
    for row in &mut out {
        let mut starts: Vec<i32> = Vec::with_capacity(row.char_starts.len());
        let mut ends: Vec<i32> = Vec::with_capacity(row.char_ends.len());
        for (s, e) in row.char_starts.iter().zip(&row.char_ends) {
            let s = source[*s as usize] as i32;
            // The char holding the last folded char: a span ending inside an
            // expansion (the `o` of `œ`) keeps that source char.
            let e = source[*e as usize - 1] as i32 + 1;
            match ends.last_mut() {
                Some(last) if s < *last => *last = (*last).max(e),
                _ => {
                    starts.push(s);
                    ends.push(e);
                }
            }
        }
        row.char_starts = starts;
        row.char_ends = ends;
        row.mentions = disjoint(std::mem::take(&mut row.mentions));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_key_depouille_les_tetes() {
        assert_eq!(resolve_key("SAS BE LIVE HOTELS"), "be live hotels");
        assert_eq!(resolve_key("S.A.S. NACC"), "nacc");
        assert_eq!(
            resolve_key("la société par actions simplifiée Vinci Construction"),
            "vinci construction"
        );
        assert_eq!(resolve_key("Société MONTDIS"), "montdis");
        assert_eq!(
            resolve_key("Caisse primaire d'assurance maladie de Lille"),
            "caisse primaire d'assurance maladie de lille"
        );
        assert_eq!(resolve_key("SELARL"), "selarl");
        assert_eq!(resolve_key("la SOCIÉTÉ Yoplait France"), "yoplait france");
        assert_eq!(
            resolve_key("l'association Les Amis"),
            "association les amis"
        );
    }

    #[test]
    fn nature_par_quality_et_tete() {
        assert_eq!(nature("counsel_name", "Me Dupont"), Some(Nature::Physique));
        assert_eq!(
            nature("law_firm", "SCP Fabiani, Luc-Thaler"),
            Some(Nature::MoralePrivee)
        );
        assert_eq!(
            nature("party", "la commune de Villeurbanne"),
            Some(Nature::MoralePublique)
        );
        assert_eq!(
            nature("party", "le ministre de l'intérieur"),
            Some(Nature::MoralePublique)
        );
        assert_eq!(
            nature("party", "société Yoplait France"),
            Some(Nature::MoralePrivee)
        );
        assert_eq!(
            nature("party", "l'association Les Amis de la Terre"),
            Some(Nature::MoralePrivee)
        );
        assert_eq!(nature("party", "Yoplait France"), None);
        assert_eq!(nature("party", "la régie des transports"), None);
    }

    #[test]
    fn evidence_spans_exact_et_runs_espaces() {
        let text = "La SOCIÉTÉ  Yoplait\nFrance demande. La société Yoplait France gagne.";
        let folded: Vec<char> = fold_chars(text).chars().collect();
        let (s, e) = evidence_spans(&folded, "société Yoplait France");
        assert_eq!(s.len(), 2);
        assert_eq!((s[0], e[0]), (3, 26));
        let verbatim: String = text
            .chars()
            .skip(s[1] as usize)
            .take((e[1] - s[1]) as usize)
            .collect();
        assert_eq!(verbatim, "société Yoplait France");
    }

    #[test]
    fn a_value_too_long_for_a_name_is_no_actor() {
        let values = vec![
            "[R] [J] ".repeat(200).trim_end().to_string(),
            "société Yoplait France".to_string(),
        ];
        let rows = actor_rows("", &[Cell::new("party", Some("applicant"), &values)]);
        let kept: Vec<&str> = rows.iter().map(|r| r.value.as_str()).collect();
        assert_eq!(kept, ["société Yoplait France"]);
    }

    #[test]
    fn les_spans_sortent_en_chars_source_quand_le_pliage_s_allonge() {
        let text = "Le BŒUF et la société Cœur de Lœw gagnent.";
        let value = vec!["société Cœur de Lœw".to_string()];
        let rows = actor_rows(text, &[Cell::new("party", Some("applicant"), &value)]);
        let (s, e) = (
            rows[0].char_starts[0] as usize,
            rows[0].char_ends[0] as usize,
        );
        let verbatim: String = text.chars().skip(s).take(e - s).collect();
        assert_eq!(verbatim, "société Cœur de Lœw");
    }

    #[test]
    fn un_span_qui_finit_dans_une_expansion_garde_son_char_source() {
        let text = "O contre la maîtrise d'œuvre et Œ.";
        let value = vec!["O".to_string()];
        let rows = actor_rows(text, &[Cell::new("party", Some("applicant"), &value)]);
        let r = &rows[0];
        let spans: Vec<(i32, i32)> = r
            .char_starts
            .iter()
            .copied()
            .zip(r.char_ends.iter().copied())
            .collect();
        assert!(spans.len() > 1, "{spans:?}");
        let mut prev_end = i32::MIN;
        for (s, e) in &spans {
            assert!(s < e && *s >= prev_end, "{spans:?}");
            prev_end = *e;
        }
    }

    #[test]
    fn evidence_spans_absent_metadonnee() {
        let folded: Vec<char> = fold_chars("aucun rapport ici").chars().collect();
        let (s, e) = evidence_spans(&folded, "Me Ludivine Pontmercy");
        assert!(s.is_empty() && e.is_empty());
    }

    /// L'arête `party_ord` : le conseil pointe la partie que le connecteur qui
    /// l'ouvre nomme à sa gauche, et la nature de cellule prime la lecture de
    /// la tête de dénomination, qu'un pseudonyme n'a pas.
    #[test]
    fn l_arete_pointe_la_partie_du_connecteur() {
        let texte = "APPELANTE : SAS Gondrand, représentée par Me Javert INTIMÉE : \
                     SA Verrerie du Nord, représentée par Me Grantaire";
        let app = vec!["SAS Gondrand".to_string()];
        let def = vec!["SA Verrerie du Nord".to_string()];
        let ca = vec!["Javert".to_string()];
        let cd = vec!["Grantaire".to_string()];
        let cells = [
            Cell::new("party", Some("applicant"), &app),
            Cell::new("party", Some("defendant"), &def),
            Cell::new("counsel_name", Some("applicant"), &ca),
            Cell::new("counsel_name", Some("defendant"), &cd),
        ];
        let rows = actor_rows(texte, &cells);
        let de = |v: &str| {
            rows.iter()
                .find(|r| r.value == v)
                .map(|r| r.party_ord)
                .unwrap()
        };
        assert_eq!(de("Javert"), Some(0));
        assert_eq!(de("Grantaire"), Some(1));
        assert_eq!(de("SAS Gondrand"), None);

        let indiv = vec!["A B".to_string()];
        let cells = [Cell::physique("party", Some("applicant"), &indiv)];
        let rows = actor_rows("Par une requête, M. A B, représenté par Me Lemaire", &cells);
        assert_eq!(rows[0].nature, Some(Nature::Physique));
    }

    fn counsel(text: &str, value: &str) -> (String, Option<String>, Option<&'static str>) {
        let folded: Vec<char> = fold_chars(text).chars().collect();
        let (s, e) = evidence_spans(&folded, value);
        counsel_evidence(&folded, value, &s, &e)
    }

    #[test]
    fn counsel_evidence_prenom_en_apposition() {
        let (key, _, _) = counsel(
            "représentée par Me Laura JAVERT, avocat au barreau de MÂCON",
            "Javert",
        );
        assert_eq!(key, "laura javert");
        let (key, _, _) = counsel("assisté de Maître Jean Pierre MYRIEL, avocat", "Myriel");
        assert_eq!(key, "jean pierre myriel");
        let (key, _, _) = counsel("Mme A, représentée par Me Merll, demande", "Merll");
        assert_eq!(key, "merll");
        let (key, _, _) = counsel(
            "par Me Aline JONDRETTE-MABEUF, avocat",
            "Aline JONDRETTE-MABEUF",
        );
        assert_eq!(key, "aline jondrette-mabeuf");
        let (key, barreau, _) = counsel("aucun rapport ici", "Dupont");
        assert_eq!(key, "dupont");
        assert_eq!(barreau, None);
    }

    #[test]
    fn counsel_evidence_barreau_en_apposition() {
        let (_, b, _) = counsel(
            "Me Max ENJOLRAS de la SELARL BRUMAIRE, avocat au barreau de ROUEN, plaidant",
            "Enjolras",
        );
        assert_eq!(b.as_deref(), Some("rouen"));
        let (_, b, _) = counsel(
            "la SCP GILLENORMAND, représentée par Me GILLENORMAND, avocats au barreau des ARDENNES",
            "Gillenormand",
        );
        assert_eq!(b.as_deref(), Some("ardennes"));
        let (_, b, _) = counsel(
            "Me François GAVROCHE, avocat au barreau de BOBIGNY",
            "François GAVROCHE",
        );
        assert_eq!(b.as_deref(), Some("seine-saint-denis"));
        let (_, b, _) = counsel("Me X, avocat au barreau de l'ESSONNE, substitué par", "X");
        assert_eq!(b.as_deref(), Some("essonne"));
        let (_, b, _) = counsel("Me Y, avocat au barreau de RURITANIE", "Y");
        assert_eq!(b, None);
    }

    #[test]
    fn counsel_barreau_outremer_par_ville_ou_ressort() {
        for (texte, attendu) in [
            ("Me A, avocat au barreau de CAYENNE", "guyane"),
            ("Me A, avocat au barreau de la GUYANE", "guyane"),
            (
                "Me A, avocat au barreau de FORT-DE-FRANCE",
                "fort-de-france-(martinique)",
            ),
            (
                "Me A, avocat au barreau de POINTE-A-PITRE",
                "guadeloupe,-saint-martin,-saint-barthelemy",
            ),
            ("Me A, avocat au barreau de MAMOUDZOU", "mayotte"),
            (
                "Me A, avocat au barreau de SAINT-DENIS",
                "saint-denis-de-la-reunion",
            ),
            (
                "Me A, avocat au barreau de SAINT-PIERRE DE LA REUNION",
                "saint-pierre-de-la-reunion",
            ),
        ] {
            let (_, b, _) = counsel(texte, "A");
            assert_eq!(b.as_deref(), Some(attendu), "{texte}");
        }
    }

    #[test]
    fn counsel_role_substitution() {
        let text =
            "les observations de Me Brevet, substituant Me Chenildieu, représentant le préfet";
        let (_, _, r) = counsel(text, "Brevet");
        assert_eq!(r, Some("substituant"));
        let (_, _, r) = counsel(text, "Chenildieu");
        assert_eq!(r, Some("substitue"));
        let text =
            "Me Jenny FANTINE, avocat au barreau de DRAGUIGNAN substitué par Me Eve TOUSSAINT";
        let (_, _, r) = counsel(text, "Jenny FANTINE");
        assert_eq!(r, Some("substitue"));
        let (_, _, r) = counsel(text, "Eve TOUSSAINT");
        assert_eq!(r, Some("substituant"));
        let (_, _, r) = counsel("représentée par Me Merll, demande au tribunal", "Merll");
        assert_eq!(r, None);
    }

    #[test]
    fn counsel_role_postulant_plaidant_sans_fuite() {
        let text = "Me Pascal COURFEYRAC, avocat au barreau de MACON, postulant, assistée de \
                    Me Florence BAHOREL, avocat au barreau de LYON, plaidant";
        let (_, _, r) = counsel(text, "Pascal COURFEYRAC");
        assert_eq!(r, Some("postulant"));
        let (_, _, r) = counsel(text, "Florence BAHOREL");
        assert_eq!(r, Some("plaidant"));
        let (_, b, _) = counsel(text, "Pascal COURFEYRAC");
        assert_eq!(b.as_deref(), Some("macon"));
    }

    #[test]
    fn cotes_contradictoires_fusionnes() {
        let applicant = vec!["Chenildieu".to_string()];
        let defendant = vec!["CHENILDIEU".to_string(), "Brevet".to_string()];
        let cells: Vec<Cell<'_>> = vec![
            Cell::new("counsel_name", Some("applicant"), &applicant),
            Cell::new("counsel_name", Some("defendant"), &defendant),
        ];
        let rows = actor_rows(
            "les observations de Me Brevet, substituant Me Chenildieu",
            &cells,
        );
        let chenildieu: Vec<_> = rows
            .iter()
            .filter(|r| lj_fold::fold(&r.value) == "chenildieu")
            .collect();
        assert_eq!(chenildieu.len(), 1);
        assert_eq!(chenildieu[0].side, None);
        assert_eq!(chenildieu[0].role, Some("substitue"));
        let brevet = rows.iter().find(|r| r.value == "Brevet").unwrap();
        assert_eq!(brevet.side, Some("defendant"));
        assert_eq!(brevet.role, Some("substituant"));
    }

    #[test]
    fn an_actor_keeps_the_mentions_the_scan_read_it_from() {
        let text = "Par une requête, M. A, représenté par Me Jean-Claude Lemaire de la SELARL Lemaire Avocats, demande à la cour d'annuler l'arrêté. M. A soutient à tort qu'il a été notifié à la SCI A.";
        let scan = crate::scan::scan(text);
        let r = scan.party_registry();
        let individuals = r.view(Some(Side::Applicant), Quality::Individual);
        let counsel = vec![
            "Jean-Claude Lemaire".to_string(),
            "Me Boulanger".to_string(),
        ];
        let cells = [
            Cell::physique("party", Some("applicant"), &individuals),
            Cell::new("counsel_name", Some("applicant"), &counsel),
        ];
        let rows = actor_rows_scanned(&scan, &cells);
        let chars: Vec<char> = text.chars().collect();
        let read = |row: &ActorRow| -> Vec<String> {
            row.mentions
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[s, e]| chars[*s as usize..*e as usize].iter().collect())
                .collect()
        };
        let a = rows.iter().find(|r| r.value == "A").unwrap();
        assert!(a.char_starts.len() > 3);
        assert_eq!(read(a), ["A"]);
        let lemaire = rows
            .iter()
            .find(|r| r.value == "Jean-Claude Lemaire")
            .unwrap();
        assert!(read(lemaire)[0].starts_with("Jean-Claude Lemaire"));
        let boulanger = rows.iter().find(|r| r.value == "Me Boulanger").unwrap();
        assert!(boulanger.mentions.is_empty());
    }

    #[test]
    fn mentions_come_out_sorted_and_disjoint() {
        assert_eq!(disjoint(vec![40, 46, 3, 9, 5, 12, 3, 9]), [3, 12, 40, 46]);
    }

    /// Construit les lignes d'un côté demandeur et rend, par valeur counsel, la
    /// valeur du cabinet apposé (déréférencée via `firm_ord`).
    fn appose(text: &str, names: &[&str], firms: &[&str]) -> Vec<(String, Option<String>)> {
        let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let firms: Vec<String> = firms.iter().map(|s| s.to_string()).collect();
        let cells: Vec<Cell<'_>> = vec![
            Cell::new("law_firm", Some("applicant"), &firms),
            Cell::new("counsel_name", Some("applicant"), &names),
        ];
        let rows = actor_rows(text, &cells);
        rows.iter()
            .filter(|r| r.quality == "counsel_name")
            .map(|r| {
                (
                    r.value.clone(),
                    r.firm_ord.map(|o| rows[o as usize].value.clone()),
                )
            })
            .collect()
    }

    #[test]
    fn apposition_cabinet_par_connecteur() {
        let text = "Représenté par Me Dominique LE GUILLOU de la SELARL SELARL LE GUILLOU \
                    RODRIGUES, Plaidant, avocat au barreau de QUIMPER. Représentée par \
                    Me Lydie LAPOUS de la SELARL LEMASSON & ASSOCIES, avocat au barreau \
                    de SAINT-BRIEUC";
        let got = appose(
            text,
            &["Dominique LE GUILLOU", "Lydie LAPOUS"],
            &["SELARL LE GUILLOU RODRIGUES", "SELARL LEMASSON & ASSOCIES"],
        );
        assert_eq!(
            got,
            vec![
                (
                    "Dominique LE GUILLOU".to_string(),
                    Some("SELARL LE GUILLOU RODRIGUES".to_string())
                ),
                (
                    "Lydie LAPOUS".to_string(),
                    Some("SELARL LEMASSON & ASSOCIES".to_string())
                ),
            ]
        );
    }

    #[test]
    fn apposition_cabinet_bornee_par_le_voisin() {
        let text = "Me Alexis GRIMAUD, Me Olivia EMIN de la SELARL EYDOUX MODELSKI";
        let got = appose(
            text,
            &["Alexis GRIMAUD", "Olivia EMIN"],
            &["SELARL EYDOUX MODELSKI"],
        );
        assert_eq!(got[0].1, None);
        assert_eq!(got[1].1.as_deref(), Some("SELARL EYDOUX MODELSKI"));

        let got = appose(
            "Me Alexis GRIMAUD SELARL EYDOUX MODELSKI",
            &["Alexis GRIMAUD"],
            &["SELARL EYDOUX MODELSKI"],
        );
        assert_eq!(got[0].1, None);

        let loin = format!(
            "Me Alexis GRIMAUD de la {} SELARL EYDOUX MODELSKI",
            "x ".repeat(40)
        );
        let got = appose(&loin, &["Alexis GRIMAUD"], &["SELARL EYDOUX MODELSKI"]);
        assert_eq!(got[0].1, None);
    }

    #[test]
    fn apposition_cabinet_connecteurs_et_absence() {
        let got = appose(
            "Me Paul BERAUDO de l'AARPI MEROTTO AVOCATS",
            &["Paul BERAUDO"],
            &["AARPI MEROTTO AVOCATS"],
        );
        assert_eq!(got[0].1.as_deref(), Some("AARPI MEROTTO AVOCATS"));

        let got = appose("Me Paul BERAUDO de la SCP INCONNUE", &["Paul BERAUDO"], &[]);
        assert_eq!(got[0].1, None);
    }
}
