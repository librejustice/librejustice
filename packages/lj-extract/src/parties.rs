//! Grain acteur de `decision_party` (ADR 0182) : à partir des 7 cellules NER
//! plates (la projection), construit les lignes canoniques — valeur, qualité,
//! côté, `resolve_key`, `nature` (axe 1 de l'ontologie 0180) et
//! spans-évidences (`char_starts`/`char_ends`, codepoints sur `full_text`,
//! convention 0143).
//!
//! Les spans se dérivent par matching replié déterministe de la valeur dans
//! le texte (fold_stable, runs d'espaces équivalents) — le même algorithme
//! que l'auto-fill des gold spans (plan 2026-07-09) : moteur et gold parlent
//! la même évidence. Une valeur de provenance métadonnée sans occurrence
//! corps a des spans vides.
//!
//! Le pliage et la clé de résolution sont calculés ICI (le SQL ne replie
//! jamais) — même pliage que le chargeur de registres (ADR 0179/0181).
//!
//! Pour `counsel_name`, la clé s'enrichit des évidences d'apposition
//! (ADR 0188) : prénom devant l'occurrence → clé nom complet, barreau après
//! l'occurrence → colonne `barreau` (slug officiel CNB). Le rôle explicite
//! (substituant/substitué, postulant/plaidant) se capte au même endroit et
//! les côtés contradictoires fusionnent en côté indéterminé (ADR 0192).

use crate::compiled::fold_stable;

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

/// Pliage canonique (fold_stable + blancs réduits) — identique au chargeur
/// de registres.
fn canon(s: &str) -> String {
    fold_stable(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Clé de résolution : pliage canonique, tête dépouillée (article, sigle de
/// forme, « société » et ses qualificatifs). Vide après dépouille → repli
/// sur le pliage entier (une valeur qui n'est QU'une forme reste elle-même).
pub fn resolve_key(value: &str) -> String {
    let c = canon(value);
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

/// Slugs officiels des 164 barreaux du registre CNB — segment barreau des uid
/// `cnb:<barreau>:<nom-prenom>` (ADR 0188). Table de normalisation embarquée
/// (même statut que `FORM_SIGLES`) ; à rafraîchir si un rechargement CNB fait
/// apparaître un barreau.
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
/// (départemental ou composé) correspondant.
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
    ("cayenne", "guyane"),
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
    ("fort-de-france", "fort-de-france-(martinique)"),
    ("gap", "hautes-alpes"),
    ("guadeloupe", "guadeloupe,-saint-martin,-saint-barthelemy"),
    ("gueret", "creuse"),
    ("la-reunion", "saint-denis-de-la-reunion"),
    ("la-rochelle", "la-rochelle-rochefort"),
    ("le-puy", "haute-loire"),
    ("le-puy-en-velay", "haute-loire"),
    ("lons-le-saunier", "jura"),
    ("martinique", "fort-de-france-(martinique)"),
    ("mende", "lozere"),
    ("montauban", "tarn-et-garonne"),
    ("nanterre", "hauts-de-seine"),
    ("niort", "deux-sevres"),
    ("noumea", "noumea-(nouvelle-caledonie)"),
    ("nouvelle-caledonie", "noumea-(nouvelle-caledonie)"),
    ("papeete", "papeete---tahiti-(nouvelle-caledonie)"),
    ("perpignan", "pyrenees-orientales"),
    ("pontoise", "val-d-oise"),
    ("privas", "ardeche"),
    ("rochefort", "la-rochelle-rochefort"),
    ("rodez", "aveyron"),
    ("sables-d-olonne", "les-sables-d-olonne"),
    ("saint-denis", "saint-denis-de-la-reunion"),
    ("saint-malo", "saint-malo-dinan"),
    ("sarlat", "bergerac-sarlat"),
    ("tahiti", "papeete---tahiti-(nouvelle-caledonie)"),
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

/// Nature de l'acteur (axe 1 fermé, ADR 0180). `None` = le moteur ne se
/// prononce pas (règle #12 : pas de classification forcée).
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
    let c = canon(value);
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
/// codepoints (fold_stable est 1:1 char-stable — les indices sont ceux de
/// `full_text`). Un espace de la valeur apparie un run d'espaces du texte
/// (les valeurs de métadonnée sont à blancs simples, le texte non).
/// Non-chevauchant glouton gauche→droite.
pub fn evidence_spans(folded: &[char], value: &str) -> (Vec<i32>, Vec<i32>) {
    let needle: Vec<char> = fold_stable(value).chars().collect();
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

/// Évidences de résolution d'un avocat (ADR 0188) : clé nom complet quand un
/// prénom est trouvé en apposition devant une occurrence (« Me Laura JAVERT »
/// pour la valeur `Javert`), slug du barreau en apposition après, rôle
/// explicite (ADR 0192). Une valeur nom-seul sans prénom garde sa clé
/// nom-seul — structurellement irrésoluble contre les dénominations
/// complètes du registre.
fn counsel_evidence(
    folded: &[char],
    value: &str,
    starts: &[i32],
    ends: &[i32],
) -> (String, Option<String>, Option<&'static str>) {
    let c = canon(value);
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

/// Rôle explicite en apposition (ADR 0192). La substitution encadre le nom :
/// « Me X, substituant Me Y » fait de X le `substituant` (présent à
/// l'audience) et de Y le `substitue` (titulaire du dossier) ; « substitué
/// par Me Z » inverse la lecture. `postulant`/`plaidant` suivent le nom
/// (style CA). La fenêtre avant s'arrête au prochain « Me »/« Maître » pour
/// ne jamais lire le marqueur de l'avocat voisin.
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
            .map_or(k, |(_, c)| *c)
            .to_string()
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

/// Une ligne `decision_party` au grain acteur, prête à persister.
#[derive(Debug, Clone)]
pub struct ActorRow {
    pub quality: &'static str,
    pub side: Option<&'static str>,
    pub value: String,
    pub resolve_key: String,
    pub nature: Option<Nature>,
    pub barreau: Option<String>,
    /// `substituant` | `substitue` | `postulant` | `plaidant` (ADR 0192),
    /// `None` = aucun marqueur.
    pub role: Option<&'static str>,
    pub char_starts: Vec<i32>,
    pub char_ends: Vec<i32>,
}

/// Cellule NER plate : (qualité, côté, valeurs) — l'ordre des cellules
/// définit `ord` intra-décision, comme le backfill 0181.
pub type Cell<'a> = (&'static str, Option<&'static str>, &'a [String]);

/// Construit les lignes acteur d'une décision depuis ses cellules plates.
/// Le texte est plié UNE fois ; chaque valeur reçoit ses spans-évidences,
/// sa nature et sa clé de résolution.
pub fn actor_rows(full_text: &str, cells: &[Cell<'_>]) -> Vec<ActorRow> {
    actor_rows_folded(&fold_stable(full_text), cells)
}

/// Variante à texte déjà plié (`DocScan::folded`, chemin prod) — évite un
/// second pliage par décision.
pub fn actor_rows_folded(folded: &str, cells: &[Cell<'_>]) -> Vec<ActorRow> {
    let folded: Vec<char> = folded.chars().collect();
    let mut rows = Vec::new();
    for (quality, side, values) in cells {
        for value in *values {
            let (char_starts, char_ends) = evidence_spans(&folded, value);
            let (key, barreau, role) = if *quality == "counsel_name" {
                counsel_evidence(&folded, value, &char_starts, &char_ends)
            } else {
                (resolve_key(value), None, None)
            };
            rows.push(ActorRow {
                quality,
                side: *side,
                value: value.clone(),
                resolve_key: key,
                nature: nature(quality, value),
                barreau,
                role,
                char_starts,
                char_ends,
            });
        }
    }
    // Cohérence de côté (ADR 0192) : une même valeur counsel émise des deux
    // côtés par le NER est du bruit — un avocat ne représente jamais les
    // deux parties. Une seule ligne, côté indéterminé ; spans identiques
    // par construction (même valeur pliée), premier rôle non-nul conservé.
    let mut out: Vec<ActorRow> = Vec::with_capacity(rows.len());
    let mut counsel_at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in rows {
        if r.quality != "counsel_name" {
            out.push(r);
            continue;
        }
        match counsel_at.entry(canon(&r.value)) {
            std::collections::hash_map::Entry::Occupied(e) => {
                let prev = &mut out[*e.get()];
                if prev.side != r.side {
                    prev.side = None;
                }
                if prev.role.is_none() {
                    prev.role = r.role;
                }
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(out.len());
                out.push(r);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_key_depouille_les_tetes() {
        // sigle de forme (avec ou sans points)
        assert_eq!(resolve_key("SAS BE LIVE HOTELS"), "be live hotels");
        assert_eq!(resolve_key("S.A.S. NACC"), "nacc");
        // article + société + qualificatifs développés
        assert_eq!(
            resolve_key("la société par actions simplifiée Vinci Construction"),
            "vinci construction"
        );
        assert_eq!(resolve_key("Société MONTDIS"), "montdis");
        // rien à dépouiller : le pliage canonique seul
        assert_eq!(
            resolve_key("Caisse primaire d'assurance maladie de Lille"),
            "caisse primaire d'assurance maladie de lille"
        );
        // valeur réduite à une forme : repli sur le pliage entier
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
        // Tête inconnue : le moteur ne se prononce pas.
        assert_eq!(nature("party", "Yoplait France"), None);
        // « régiment »/« régie » ne matchent pas « region  » (frontière).
        assert_eq!(nature("party", "la régie des transports"), None);
    }

    #[test]
    fn evidence_spans_exact_et_runs_espaces() {
        let text = "La SOCIÉTÉ  Yoplait\nFrance demande. La société Yoplait France gagne.";
        let folded: Vec<char> = fold_stable(text).chars().collect();
        let (s, e) = evidence_spans(&folded, "société Yoplait France");
        assert_eq!(s.len(), 2);
        // 1ʳᵉ occurrence : double espace + saut de ligne appariés.
        assert_eq!((s[0], e[0]), (3, 26));
        let verbatim: String = text
            .chars()
            .skip(s[1] as usize)
            .take((e[1] - s[1]) as usize)
            .collect();
        assert_eq!(verbatim, "société Yoplait France");
    }

    #[test]
    fn evidence_spans_absent_metadonnee() {
        let folded: Vec<char> = fold_stable("aucun rapport ici").chars().collect();
        let (s, e) = evidence_spans(&folded, "Me Ludivine Pontmercy");
        assert!(s.is_empty() && e.is_empty());
    }

    fn counsel(text: &str, value: &str) -> (String, Option<String>, Option<&'static str>) {
        let folded: Vec<char> = fold_stable(text).chars().collect();
        let (s, e) = evidence_spans(&folded, value);
        counsel_evidence(&folded, value, &s, &e)
    }

    #[test]
    fn counsel_evidence_prenom_en_apposition() {
        // Prénom capté devant l'occurrence → clé nom complet.
        let (key, _, _) = counsel(
            "représentée par Me Laura JAVERT, avocat au barreau de MÂCON",
            "Javert",
        );
        assert_eq!(key, "laura javert");
        // Prénom composé (2 tokens).
        let (key, _, _) = counsel("assisté de Maître Jean Pierre MYRIEL, avocat", "Myriel");
        assert_eq!(key, "jean pierre myriel");
        // Pas de prénom entre le marqueur et la valeur → clé nom-seul.
        let (key, _, _) = counsel("Mme A, représentée par Me Merll, demande", "Merll");
        assert_eq!(key, "merll");
        // Valeur déjà nom complet : pliage canonique tel quel (ordre du texte).
        let (key, _, _) = counsel(
            "par Me Aline JONDRETTE-MABEUF, avocat",
            "Aline JONDRETTE-MABEUF",
        );
        assert_eq!(key, "aline jondrette-mabeuf");
        // Sans occurrence corps : clé nom-seul, pas de barreau.
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
        // Préposition « des » + slug composé.
        let (_, b, _) = counsel(
            "la SCP GILLENORMAND, représentée par Me GILLENORMAND, avocats au barreau des ARDENNES",
            "Gillenormand",
        );
        assert_eq!(b.as_deref(), Some("ardennes"));
        // Alias ville-siège → barreau départemental.
        let (_, b, _) = counsel(
            "Me François GAVROCHE, avocat au barreau de BOBIGNY",
            "François GAVROCHE",
        );
        assert_eq!(b.as_deref(), Some("seine-saint-denis"));
        // « de l' » + frontière (le texte continue après le slug).
        let (_, b, _) = counsel("Me X, avocat au barreau de l'ESSONNE, substitué par", "X");
        assert_eq!(b.as_deref(), Some("essonne"));
        // Barreau inconnu du registre → None.
        let (_, b, _) = counsel("Me Y, avocat au barreau de RURITANIE", "Y");
        assert_eq!(b, None);
    }

    #[test]
    fn counsel_role_substitution() {
        let text =
            "les observations de Me Brevet, substituant Me Chenildieu, représentant le préfet";
        // Le substituant est suivi du marqueur.
        let (_, _, r) = counsel(text, "Brevet");
        assert_eq!(r, Some("substituant"));
        // Le titulaire est précédé de « substituant Me ».
        let (_, _, r) = counsel(text, "Chenildieu");
        assert_eq!(r, Some("substitue"));
        // Construction inverse « substitué par ».
        let text =
            "Me Jenny FANTINE, avocat au barreau de DRAGUIGNAN substitué par Me Eve TOUSSAINT";
        let (_, _, r) = counsel(text, "Jenny FANTINE");
        assert_eq!(r, Some("substitue"));
        let (_, _, r) = counsel(text, "Eve TOUSSAINT");
        assert_eq!(r, Some("substituant"));
        // Aucun marqueur → None.
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
        // La fenêtre de COURFEYRAC s'arrête au « Me » suivant : jamais le marqueur
        // du voisin. Symétriquement BAHOREL ne voit pas « postulant ».
        let (_, b, _) = counsel(text, "Pascal COURFEYRAC");
        assert_eq!(b.as_deref(), Some("macon"));
    }

    #[test]
    fn cotes_contradictoires_fusionnes() {
        let applicant = vec!["Chenildieu".to_string()];
        let defendant = vec!["CHENILDIEU".to_string(), "Brevet".to_string()];
        let cells: Vec<Cell<'_>> = vec![
            ("counsel_name", Some("applicant"), &applicant),
            ("counsel_name", Some("defendant"), &defendant),
        ];
        let rows = actor_rows(
            "les observations de Me Brevet, substituant Me Chenildieu",
            &cells,
        );
        // Chenildieu émis des deux côtés → UNE ligne, côté indéterminé.
        let chenildieu: Vec<_> = rows
            .iter()
            .filter(|r| canon(&r.value) == "chenildieu")
            .collect();
        assert_eq!(chenildieu.len(), 1);
        assert_eq!(chenildieu[0].side, None);
        assert_eq!(chenildieu[0].role, Some("substitue"));
        // Brevet garde son côté.
        let brevet = rows.iter().find(|r| r.value == "Brevet").unwrap();
        assert_eq!(brevet.side, Some("defendant"));
        assert_eq!(brevet.role, Some("substituant"));
    }
}
