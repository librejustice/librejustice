//! Signaux structurés d'une clé de citation (ADR 0144, `citation_key`).
//! Regexes sur la CHAÎNE DE CLÉ (jamais le texte) — linking ADR 0116.
//!
//! `key_signals(text_key)` est une **fonction pure de la chaîne de clé seule**
//! — jamais du contexte de capture : c'est le contexte (raw_text première
//! frappe) qui a produit la pourriture first-writer-wins de l'ancien
//! vocabulaire. Les règles du résolveur (DatedAct, EuNum, ForeignCode…)
//! matchent sur CES signaux, plus jamais par regex sur de la prose.
//!
//! Versionné par [`SIGNAL_VERSION`] : une évolution du parse se rejoue par
//! `reparse-key-signals` sur les clés à version inférieure.

use std::sync::OnceLock;

use jiff::civil::Date;
use regex::Regex;

use super::instruments::FOREIGN_NATIONALITY_STEMS;
use super::is_unresolvable_instrument;
use super::text::fold;

/// Version du parse des signaux (colonne `citation_key.signal_version`).
pub const SIGNAL_VERSION: i16 = 1;

/// Famille d'instrument, discriminants stables = valeurs de la colonne
/// `citation_key.nature` (int2). Ne jamais renuméroter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i16)]
pub enum KeyNature {
    Autre = 0,
    Code = 1,
    CodeEtranger = 2,
    Loi = 3,
    Decret = 4,
    Arrete = 5,
    Ordonnance = 6,
    Deliberation = 7,
    Constitution = 8,
    ReglementUe = 9,
    DirectiveUe = 10,
    TraiteAccord = 11,
    Ccn = 12,
}

/// Gate de citabilité (ADR 0137), discriminants stables = valeurs de la
/// colonne `citation_key.citability` (int2). ≠ 0 ⇒ le résolveur saute la clé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i16)]
pub enum Citability {
    Citable = 0,
    /// Acte local non catalogué : PLU, arrêté municipal/préfectoral…
    LocalAct = 1,
    /// Norme privée : statuts, règlement intérieur/de copropriété, CGV…
    PrivateStatut = 2,
    /// Capture sans identité résoluble (famille nue, anaphore orpheline…).
    Fragment = 3,
}

/// Signaux structurés d'une clé. `jurisdiction` = ISO-2 quand un gentilé
/// étranger identifie le pays SANS ambiguïté (« congolais » → `None`, CD/CG
/// indécidable au grain clé — c'est le travail contextuel de l'oracle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySignals {
    pub nature: KeyNature,
    pub jurisdiction: Option<&'static str>,
    pub act_date: Option<Date>,
    pub act_num: Option<String>,
    pub citability: Citability,
}

/// Parse les signaux d'une clé canonique (`text_key`, sortie de
/// `normalize_instrument`). Déterministe, total (jamais d'erreur).
pub fn key_signals(text_key: &str) -> KeySignals {
    let folded = fold(text_key);
    let nature = classify_nature(&folded);
    let jurisdiction = foreign_jurisdiction(&folded);
    KeySignals {
        nature,
        jurisdiction,
        act_date: parse_act_date(&folded),
        act_num: parse_act_num(&folded, nature),
        citability: classify_citability(text_key, &folded, nature),
    }
}

fn classify_nature(folded: &str) -> KeyNature {
    // Ordre : les familles à préfixe ambigu se testent du plus spécifique au
    // plus général (« convention collective » avant « convention »).
    if folded.starts_with("code") || folded.starts_with("livre des procedures fiscales") {
        if foreign_jurisdiction_word(folded) {
            return KeyNature::CodeEtranger;
        }
        return KeyNature::Code;
    }
    if folded.starts_with("constitution") {
        return KeyNature::Constitution;
    }
    if folded.starts_with("convention collective") || folded.contains("idcc") {
        return KeyNature::Ccn;
    }
    if folded.starts_with("loi") {
        return KeyNature::Loi;
    }
    if folded.starts_with("decret") {
        return KeyNature::Decret;
    }
    if folded.starts_with("arrete") {
        return KeyNature::Arrete;
    }
    if folded.starts_with("ordonnance") {
        return KeyNature::Ordonnance;
    }
    if folded.starts_with("deliberation") {
        return KeyNature::Deliberation;
    }
    if folded.starts_with("directive") {
        return KeyNature::DirectiveUe;
    }
    if folded.starts_with("reglement") {
        // Droit dérivé UE seulement : marqueur (UE/CE/CEE/euratom) ou numéro
        // année/séquence. « règlement de copropriété », « règlement sanitaire
        // départemental » → Autre (gated par la citabilité).
        if re_eu_marker().is_match(folded) || re_eu_num().is_match(folded) {
            return KeyNature::ReglementUe;
        }
        return KeyNature::Autre;
    }
    if folded.starts_with("convention")
        || folded.starts_with("accord")
        || folded.starts_with("traite")
        || folded.starts_with("charte")
        || folded.starts_with("pacte")
        || folded.starts_with("protocole")
        || folded.starts_with("declaration universelle")
    {
        return KeyNature::TraiteAccord;
    }
    KeyNature::Autre
}

/// Date de l'acte : premier « du <jour> <mois> <année> » de la clé — ancré en
/// tête d'identité (un avenant daté citant un texte hôte daté garde SA date).
fn parse_act_date(folded: &str) -> Option<Date> {
    let c = re_act_date().captures(folded)?;
    let day: i8 = c.get(1)?.as_str().parse().ok()?;
    let month = month_number(c.get(2)?.as_str())?;
    let year: i16 = c.get(3)?.as_str().parse().ok()?;
    Date::new(year, month, day).ok()
}

/// Numéro de l'acte selon la famille : `NN-NNN`/`NNNN-NNN` (actes FR),
/// `NNNN/NN` (droit dérivé UE), `IDCC NNNN` (CCN).
fn parse_act_num(folded: &str, nature: KeyNature) -> Option<String> {
    match nature {
        KeyNature::ReglementUe | KeyNature::DirectiveUe => re_eu_num().captures(folded).map(|c| {
            format!(
                "{}/{}",
                c.get(1).unwrap().as_str(),
                c.get(2).unwrap().as_str()
            )
        }),
        KeyNature::Ccn => re_idcc().captures(folded).map(|c| {
            let mut s = String::from("IDCC ");
            s.push_str(c.get(1).unwrap().as_str());
            s
        }),
        _ => re_fr_num()
            .captures(folded)
            .map(|c| c.get(1).unwrap().as_str().to_string()),
    }
}

fn classify_citability(text_key: &str, folded: &str, nature: KeyNature) -> Citability {
    if is_unresolvable_instrument(text_key) {
        return Citability::Fragment;
    }
    if re_local_act().is_match(folded) {
        return Citability::LocalAct;
    }
    if re_private_statut().is_match(folded) {
        return Citability::PrivateStatut;
    }
    // « Règlement » sans marqueur UE ni citabilité tranchée = règlement d'une
    // instance locale/privée non catalogable (procédure, jeu, service…) —
    // SAUF la forme légistique datée à queue de titre (« règlement du
    // 17 décembre 2013 établissant les règles… ») : c'est le droit dérivé UE
    // cité par date sans numéro, jamais le style d'un règlement privé/local.
    if nature == KeyNature::Autre
        && folded.starts_with("reglement")
        && !re_eu_dated_title().is_match(folded)
    {
        return Citability::LocalAct;
    }
    Citability::Citable
}

/// Un mot de la clé porte-t-il un gentilé étranger (même critère que la garde
/// anti-repli de `snap_code_name`) ?
fn foreign_jurisdiction_word(folded: &str) -> bool {
    folded
        .split_whitespace()
        .any(|w| FOREIGN_NATIONALITY_STEMS.iter().any(|st| w.starts_with(st)))
}

/// ISO-2 du gentilé étranger de la clé, si un pays UNIQUE en découle. Le stem
/// le plus long gagne (« nigerian » avant « nigerien » ne suffit pas : deux
/// stems peuvent matcher le même mot, ex. « nigeria(n) »).
fn foreign_jurisdiction(folded: &str) -> Option<&'static str> {
    let mut best: Option<(&'static str, Option<&'static str>)> = None;
    for w in folded.split_whitespace() {
        for (stem, iso) in GENTILE_ISO {
            if w.starts_with(stem) && best.is_none_or(|(b, _)| stem.len() > b.len()) {
                best = Some((stem, *iso));
            }
        }
    }
    best.and_then(|(_, iso)| iso)
}

/// Gentilé (stem plié, cf. `FOREIGN_NATIONALITY_STEMS`) → ISO-2. `None` =
/// ambigu ou sans État (congolais CD/CG, kurde, tibétain, « africain » nu…).
/// L'alignement 1:1 avec la liste des stems est vérifié par test.
const GENTILE_ISO: &[(&str, Option<&'static str>)] = &[
    ("iranien", Some("IR")),
    ("irakien", Some("IQ")),
    ("syrien", Some("SY")),
    ("libanai", Some("LB")),
    ("afghan", Some("AF")),
    ("pakistanai", Some("PK")),
    ("bangladai", Some("BD")),
    ("indien", Some("IN")),
    ("ivoirien", Some("CI")),
    ("guineen", Some("GN")),
    ("malien", Some("ML")),
    ("senegalai", Some("SN")),
    ("congolai", None), // CD / CG indécidable au grain clé
    ("camerounai", Some("CM")),
    ("tchadien", Some("TD")),
    ("nigerien", Some("NE")),
    ("nigeria", Some("NG")),
    ("soudanai", Some("SD")),
    ("erythreen", Some("ER")),
    ("ethiopien", Some("ET")),
    ("somalien", Some("SO")),
    ("centrafricain", Some("CF")),
    ("rwandai", Some("RW")),
    ("burundai", Some("BI")),
    ("gabonai", Some("GA")),
    ("togolai", Some("TG")),
    ("beninoi", Some("BJ")),
    ("gambien", Some("GM")),
    ("liberien", Some("LR")),
    ("ghaneen", Some("GH")),
    ("angolai", Some("AO")),
    ("mauritanien", Some("MR")),
    ("marocain", Some("MA")),
    ("tunisien", Some("TN")),
    ("algerien", Some("DZ")),
    ("libyen", Some("LY")),
    ("egyptien", Some("EG")),
    ("palestinien", Some("PS")),
    ("jordanien", Some("JO")),
    ("yemenite", Some("YE")),
    ("saoudien", Some("SA")),
    ("georgien", Some("GE")),
    ("armenien", Some("AM")),
    ("azerbaidjanai", Some("AZ")),
    ("ukrainien", Some("UA")),
    ("russe", Some("RU")),
    ("bielorusse", Some("BY")),
    ("tchetchene", None), // pas d'État
    ("albanai", Some("AL")),
    ("serbe", Some("RS")),
    ("bosnien", Some("BA")),
    ("kosovar", Some("XK")),
    ("macedonien", Some("MK")),
    ("chinoi", Some("CN")),
    ("vietnamien", Some("VN")),
    ("cambodgien", Some("KH")),
    ("birman", Some("MM")),
    ("tibetain", None), // pas d'État
    ("mongol", Some("MN")),
    ("srilankai", Some("LK")),
    ("nepalai", Some("NP")),
    ("bhoutanai", Some("BT")),
    ("ouzbek", Some("UZ")),
    ("tadjik", Some("TJ")),
    ("kirghiz", Some("KG")),
    ("kazakh", Some("KZ")),
    ("turkmene", Some("TM")),
    ("haitien", Some("HT")),
    ("colombien", Some("CO")),
    ("venezuelien", Some("VE")),
    ("salvadorien", Some("SV")),
    ("hondurien", Some("HN")),
    ("kurde", None), // pas d'État
    ("turc", Some("TR")),
    ("turqu", Some("TR")),
    ("allemand", Some("DE")),
    ("autrichien", Some("AT")),
    ("suisse", Some("CH")),
    ("belge", Some("BE")),
    ("luxembourgeoi", Some("LU")),
    ("neerlandai", Some("NL")),
    ("hollandai", Some("NL")),
    ("espagnol", Some("ES")),
    ("portugai", Some("PT")),
    ("italien", Some("IT")),
    ("anglai", Some("GB")),
    ("britannique", Some("GB")),
    ("ecossai", Some("GB")),
    ("irlandai", Some("IE")),
    ("americain", Some("US")),
    ("canadien", Some("CA")),
    ("quebecoi", Some("CA")),
    ("bresilien", Some("BR")),
    ("argentin", Some("AR")),
    ("mexicain", Some("MX")),
    ("chilien", Some("CL")),
    ("danoi", Some("DK")),
    ("suedoi", Some("SE")),
    ("norvegien", Some("NO")),
    ("finlandai", Some("FI")),
    ("islandai", Some("IS")),
    ("polonai", Some("PL")),
    ("tcheque", Some("CZ")),
    ("slovaque", Some("SK")),
    ("slovene", Some("SI")),
    ("hongroi", Some("HU")),
    ("roumain", Some("RO")),
    ("bulgare", Some("BG")),
    ("grec", Some("GR")),
    ("croate", Some("HR")),
    ("estonien", Some("EE")),
    ("letton", Some("LV")),
    ("lituanien", Some("LT")),
    ("maltai", Some("MT")),
    ("chypriote", Some("CY")),
    ("monegasque", Some("MC")),
    ("japonai", Some("JP")),
    ("coreen", Some("KR")),
    ("indonesien", Some("ID")),
    ("thailandai", Some("TH")),
    ("philippin", Some("PH")),
    ("australien", Some("AU")),
    ("neozelandai", Some("NZ")),
    ("moldave", Some("MD")),
    ("montenegrin", Some("ME")),
    ("andorran", Some("AD")),
    ("liechtensteinoi", Some("LI")),
    ("saint-marinai", Some("SM")),
    ("israelien", Some("IL")),
    ("koweitien", Some("KW")),
    ("qatari", Some("QA")),
    ("bahreini", Some("BH")),
    ("omanai", Some("OM")),
    ("emirati", Some("AE")),
    ("africain", None), // générique (« charte africaine »…), pas un pays
    ("sud-africain", Some("ZA")),
    ("burkinabe", Some("BF")),
    ("capverdien", Some("CV")),
    ("malgache", Some("MG")),
    ("mauricien", Some("MU")),
    ("comorien", Some("KM")),
    ("seychelloi", Some("SC")),
    ("djiboutien", Some("DJ")),
    ("kenyan", Some("KE")),
    ("tanzanien", Some("TZ")),
    ("ougandai", Some("UG")),
    ("zambien", Some("ZM")),
    ("zimbabween", Some("ZW")),
    ("mozambicain", Some("MZ")),
    ("namibien", Some("NA")),
    ("botswanai", Some("BW")),
    ("swazi", Some("SZ")),
    ("nigerian", Some("NG")),
    ("sierra-leonai", Some("SL")),
    ("bissau-guineen", Some("GW")),
    ("equato-guineen", Some("GQ")),
    ("cubain", Some("CU")),
    ("dominicain", Some("DO")),
    ("jamaiquain", Some("JM")),
    ("trinidadien", Some("TT")),
    ("guatemalteque", Some("GT")),
    ("costaricien", Some("CR")),
    ("panameen", Some("PA")),
    ("nicaraguayen", Some("NI")),
    ("equatorien", Some("EC")),
    ("peruvien", Some("PE")),
    ("bolivien", Some("BO")),
    ("paraguayen", Some("PY")),
    ("uruguayen", Some("UY")),
    ("portoricain", Some("PR")),
    ("guyanien", Some("GY")),
    ("surinamai", Some("SR")),
    ("laotien", Some("LA")),
    ("malaisien", Some("MY")),
    ("singapourien", Some("SG")),
    ("bruneien", Some("BN")),
    ("taiwanai", Some("TW")),
    ("maldivien", Some("MV")),
    ("timorai", Some("TL")),
    ("qatarien", Some("QA")),
    ("fidjien", Some("FJ")),
    ("papouasien", Some("PG")),
    ("samoan", Some("WS")),
    ("tongien", Some("TO")),
];

fn month_number(m: &str) -> Option<i8> {
    Some(match m {
        "janvier" => 1,
        "fevrier" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "aout" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        "decembre" => 12,
        _ => return None,
    })
}

fn re_act_date() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // « le <date> » vaut « du <date> » dans les formes conventionnelles
        // (« convention conclue entre la France et la Suisse le 9 septembre
        // 1966 », « signée à Paris le … »).
        Regex::new(
            r"\b(?:du|le) (\d{1,2})(?:er)? (janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre) (\d{4})",
        )
        .unwrap()
    })
}

fn re_fr_num() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `fold` garde la ponctuation : « n° 91-647 », « no 91-647 », « n 91-647 ».
    RE.get_or_init(|| Regex::new(r"\bn[o°]? ?(\d{2,4}-\d+)\b").unwrap())
}

fn re_eu_num() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // OCR tolérés : « no2201/ 2003 » (n° collé au numéro, espace après la
    // barre) — le numéro est recomposé sans espace par l'appelant.
    RE.get_or_init(|| Regex::new(r"(?:\b|n[o°])(\d{1,4}) ?/ ?(\d{1,4})\b").unwrap())
}

fn re_eu_marker() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(ue|ce|cee|euratom)\b").unwrap())
}

fn re_idcc() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bidcc ?(\d{2,4})\b").unwrap())
}

fn re_local_act() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\b(municipal|prefectoral|communal|departemental|plan local d.urbanisme|plu\b|plan d.occupation des sols)",
        )
        .unwrap()
    })
}

fn re_eu_dated_title() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^reglement(?:\s*\([^)]{2,20}\))?(?:\s+du parlement europeen et du conseil|\s+du conseil|\s+de la commission)?,?\s+du \d{1,2}(?:er)? (?:janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre) \d{4}\s+(?:etablissant|portant|relatif|relative|concernant|fixant|instituant|modifiant)\b",
        )
        .unwrap()
    })
}

fn re_private_statut() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\b(reglement interieur|reglement de copropriete|statuts?\b|conditions generales|cahier des charges)",
        )
        .unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Alignement 1:1 stems ↔ map ISO : chaque stem a exactement une entrée
    // (une clé étrangère non mappée serait un trou silencieux de ForeignCode).
    #[test]
    fn gentile_iso_map_covers_every_stem() {
        for stem in FOREIGN_NATIONALITY_STEMS {
            assert_eq!(
                GENTILE_ISO.iter().filter(|(s, _)| s == stem).count(),
                1,
                "stem sans entrée ISO unique : {stem}"
            );
        }
        assert_eq!(GENTILE_ISO.len(), FOREIGN_NATIONALITY_STEMS.len());
    }

    #[test]
    fn foreign_codes_get_nature_and_jurisdiction() {
        let s = key_signals("Code civil suisse");
        assert_eq!(s.nature, KeyNature::CodeEtranger);
        assert_eq!(s.jurisdiction, Some("CH"));
        assert_eq!(s.citability, Citability::Citable);

        // Ambiguïté d'État : gentilé reconnu, pays indécidable → None.
        let s = key_signals("Code de la famille congolais");
        assert_eq!(s.nature, KeyNature::CodeEtranger);
        assert_eq!(s.jurisdiction, None);

        // Code FR : jamais de juridiction.
        let s = key_signals("Code de procédure civile");
        assert_eq!(s.nature, KeyNature::Code);
        assert_eq!(s.jurisdiction, None);
    }

    #[test]
    fn dated_act_number_and_date() {
        let s = key_signals("Loi n° 91-647 du 10 juillet 1991");
        assert_eq!(s.nature, KeyNature::Loi);
        assert_eq!(s.act_num.as_deref(), Some("91-647"));
        assert_eq!(s.act_date, Date::new(1991, 7, 10).ok());

        // « 1er » : jour ordinal.
        let s = key_signals("Décret du 1er juillet 2009");
        assert_eq!(s.nature, KeyNature::Decret);
        assert_eq!(s.act_date, Date::new(2009, 7, 1).ok());

        // La date de tête (l'avenant) gagne sur la date du texte hôte.
        let s = key_signals("Avenant du 25 août 2016 à la convention du 14 janvier 1971");
        assert_eq!(s.act_date, Date::new(2016, 8, 25).ok());
    }

    #[test]
    fn eu_secondary_law() {
        let s = key_signals("Règlement (UE) n° 1215/2012");
        assert_eq!(s.nature, KeyNature::ReglementUe);
        assert_eq!(s.act_num.as_deref(), Some("1215/2012"));

        let s = key_signals("Directive 2008/115/CE");
        assert_eq!(s.nature, KeyNature::DirectiveUe);
        assert_eq!(s.act_num.as_deref(), Some("2008/115"));

        // « Règlement » non-UE : ni nature UE, ni citable.
        let s = key_signals("Règlement de copropriété");
        assert_eq!(s.nature, KeyNature::Autre);
        assert_eq!(s.citability, Citability::PrivateStatut);
    }

    #[test]
    fn ccn_and_treaties() {
        let s = key_signals("Convention collective nationale de la métallurgie");
        assert_eq!(s.nature, KeyNature::Ccn);

        let s = key_signals("Convention de Lugano du 30 octobre 2007");
        assert_eq!(s.nature, KeyNature::TraiteAccord);
        assert_eq!(s.act_date, Date::new(2007, 10, 30).ok());
    }

    #[test]
    fn citability_gates() {
        assert_eq!(
            key_signals("Arrêté préfectoral du 3 mai 2019").citability,
            Citability::LocalAct
        );
        assert_eq!(
            key_signals("Règlement intérieur de l'entreprise").citability,
            Citability::PrivateStatut
        );
        // Fragment : famille nue (même critère que le drop du recognizer).
        assert_eq!(key_signals("Livre VIII").citability, Citability::Fragment);
    }
}
