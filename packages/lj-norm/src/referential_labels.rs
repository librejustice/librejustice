//! Libellés FR d'affichage du référentiel de textes (`legal_text` /
//! `legal_article`) — mappings purs valeur DB → libellé humain, pour les
//! facettes de `/norm-search` et le catalogue des codes.
//!
//! Trois axes :
//! - `jurisdiction` : ISO 3166-1 alpha-2 + sentinelles `EU`/`INTL` → nom FR ;
//! - `nature` : jetons LEGI (`LOI_ORGANIQUE`) et catégories curées
//!   (`code_famille`) → libellé FR — l'entrée est normalisée en majuscules
//!   (le corpus curé mélange `loi`/`LOI`, `constitution`/`CONSTITUTION`) ;
//! - `source` : les diffuseurs à jeton court (`legifrance`, `kali`, `ris-at`…)
//!   reçoivent un libellé ; un domaine (`sgg-mali.ml`, `africa-laws.org`…) se
//!   lit tel quel.
//!
//! `None` = valeur inconnue du mapping, l'appelant retombe sur la valeur brute.

/// Nom FR d'une juridiction du référentiel (code ISO alpha-2, `EU`, `INTL`).
pub fn jurisdiction_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "FR" => "France",
        "EU" => "Union européenne",
        "INTL" => "International",
        "AM" => "Arménie",
        "AO" => "Angola",
        "AT" => "Autriche",
        "BE" => "Belgique",
        "BF" => "Burkina Faso",
        "BG" => "Bulgarie",
        "BI" => "Burundi",
        "BJ" => "Bénin",
        "CD" => "RD Congo",
        "CF" => "Centrafrique",
        "CG" => "Congo",
        "CH" => "Suisse",
        "CI" => "Côte d'Ivoire",
        "CM" => "Cameroun",
        "DE" => "Allemagne",
        "DJ" => "Djibouti",
        "DO" => "République dominicaine",
        "DZ" => "Algérie",
        "EG" => "Égypte",
        "ES" => "Espagne",
        "GA" => "Gabon",
        "GN" => "Guinée",
        "GR" => "Grèce",
        "HT" => "Haïti",
        "HU" => "Hongrie",
        "IQ" => "Irak",
        "IT" => "Italie",
        "JO" => "Jordanie",
        "KM" => "Comores",
        "LB" => "Liban",
        "LU" => "Luxembourg",
        "MA" => "Maroc",
        "MC" => "Monaco",
        "MG" => "Madagascar",
        "ML" => "Mali",
        "MR" => "Mauritanie",
        "MU" => "Maurice",
        "NC" => "Nouvelle-Calédonie",
        "NE" => "Niger",
        "NG" => "Nigeria",
        "NL" => "Pays-Bas",
        "PE" => "Pérou",
        "PF" => "Polynésie française",
        "PL" => "Pologne",
        "PT" => "Portugal",
        "RO" => "Roumanie",
        "RS" => "Serbie",
        "RU" => "Russie",
        "RW" => "Rwanda",
        "SN" => "Sénégal",
        "ST" => "Sao Tomé-et-Principe",
        "SY" => "Syrie",
        "TD" => "Tchad",
        "TG" => "Togo",
        "TN" => "Tunisie",
        "TR" => "Turquie",
        "UA" => "Ukraine",
        "VE" => "Venezuela",
        "VN" => "Viêt Nam",
        _ => return None,
    })
}

/// Libellé FR d'une nature de texte. Entrée normalisée en majuscules avant
/// lookup — un appelant peut passer la valeur DB brute (`code_famille`,
/// `constitution`) comme la valeur de facette déjà normalisée (`CODE_FAMILLE`).
pub fn nature_label(nature: &str) -> Option<&'static str> {
    Some(match nature.to_uppercase().as_str() {
        "ACCORD" => "Accord",
        "ACCORD_FONCTION_PUBLIQUE" => "Accord fonction publique",
        "AMENDEMENT" => "Amendement",
        "ANNEXE" => "Annexe",
        "ARRANGEMENT" => "Arrangement",
        "ARRETE" => "Arrêté",
        "AVENANT" => "Avenant",
        "AVIS" => "Avis",
        "BOFIP" => "BOFiP (doctrine fiscale)",
        "CHARTE" => "Charte",
        "CIRCULAIRE" => "Circulaire",
        "CODE" => "Code",
        "CODE_AUTRE" => "Code (autre)",
        "CODE_CIVIL" => "Code civil",
        "CODE_COMMERCE" => "Code de commerce",
        "CODE_DIP" => "Code de droit international privé",
        "CODE_DOUANES" => "Code des douanes",
        "CODE_ETAT_CIVIL" => "Code de l'état civil",
        "CODE_FAMILLE" => "Code de la famille",
        "CODE_IMPOTS" => "Code des impôts",
        "CODE_NATIONALITE" => "Code de la nationalité",
        "CODE_OBLIGATIONS" => "Code des obligations",
        "CODE_PENAL" => "Code pénal",
        "CODE_PERSONNES" => "Code des personnes",
        "CODE_PROCEDURE" => "Code de procédure",
        "CODE_PROCEDURE_CIVILE" => "Code de procédure civile",
        "CODE_PROCEDURE_PENALE" => "Code de procédure pénale",
        "CODE_TRAVAIL" => "Code du travail",
        "COMPOSITION" => "Composition",
        "CONSTITUTION" => "Constitution",
        "CONVENTION" => "Convention",
        "DECISION" => "Décision",
        "DECLARATION" => "Déclaration",
        "DECRET" => "Décret",
        "DECRET_LOI" => "Décret-loi",
        "DELIBERATION" => "Délibération",
        "DIRECTIVE" => "Directive",
        "DIRECTIVE_EURO" => "Directive européenne",
        "ENTENTE" => "Entente",
        "ETAT_CIVIL" => "État civil",
        "FICHE_PRATIQUE" => "Fiche pratique",
        "IDCC" => "Convention collective",
        "INFORMATIONS_PARLEMENTAIRES" => "Informations parlementaires",
        "INSTRUCTION" => "Instruction",
        "LETTRE" => "Lettre",
        "LISTE" => "Liste",
        "LOI" => "Loi",
        "LOI_CONSTIT" => "Loi constitutionnelle",
        "LOI_ORGANIQUE" => "Loi organique",
        "LOI_PROGRAMME" => "Loi de programmation",
        "MEMOIRE" => "Mémoire",
        "MEMORANDUM" => "Mémorandum",
        "MODIFICATION" => "Modification",
        "ORDONNANCE" => "Ordonnance",
        "PROJET" => "Projet",
        "PROPOSITION" => "Proposition",
        "PROTOCOLE" => "Protocole",
        "RAPPORT" => "Rapport",
        "RECOMMANDATION" => "Recommandation",
        "REGLEMENT" => "Règlement",
        "RESOLUTION" => "Résolution",
        "RESULTATS" => "Résultats",
        "SAISINE" => "Saisine",
        "TI" => "Traité international",
        "TRAITE" => "Traité",
        _ => return None,
    })
}

/// Libellé FR d'un diffuseur à jeton court (`legal_article.source`). Les
/// domaines (fedlex, legilux, sgg-mali.ml…) restent leur propre libellé.
pub fn source_label(source: &str) -> Option<&'static str> {
    Some(match source {
        "legifrance" => "Légifrance",
        "bofip" => "BOFiP (DGFiP)",
        "circulaire" => "Circulaires (DILA)",
        "fiche_pratique" => "Service-Public.fr (DILA)",
        "kali" => "Conventions collectives (KALI)",
        "jorf" => "Journal officiel (JORF)",
        "jafbase" => "JAFBase",
        "eu-law" => "Droit de l'UE",
        "official-fr" => "Textes officiels (France)",
        "treaty" => "Traités",
        "mjp" => "Digithèque MJP",
        "traduction-automatique" => "Traduction automatique",
        "birosag-hu" => "Bíróság (Hongrie)",
        "ecoi" => "ECOI.net",
        "ejustice-be" => "Moniteur belge",
        "fedlex" => "Fedlex (Suisse)",
        "legilux" => "Légilux (Luxembourg)",
        "legislatie-ro" => "Legislație (Roumanie)",
        "ris-at" => "RIS (Autriche)",
        "wetten-nl" => "Wetten.nl (Pays-Bas)",
        _ => return None,
    })
}

/// The norms catalogue's fonds in display order, each text placed by `norm_fond`;
/// `codes` has its own catalogue, the others a hub per year.
pub const NORM_FONDS: &[&str] = &[
    "codes",
    "lois",
    "decrets",
    "arretes",
    "conventions-collectives",
    "textes-ue",
    "traites",
    "circulaires",
    "bofip",
    "fiches-pratiques",
    "autres",
];

/// Libellé FR d'un fond du catalogue des normes.
pub fn norm_fond_label(fond: &str) -> Option<&'static str> {
    Some(match fond {
        "codes" => "Codes",
        "lois" => "Lois et ordonnances",
        "decrets" => "Décrets",
        "arretes" => "Arrêtés",
        "conventions-collectives" => "Conventions collectives",
        "textes-ue" => "Textes de l'Union européenne",
        "traites" => "Traités et accords internationaux",
        "circulaires" => "Circulaires et instructions",
        "bofip" => "BOFiP (doctrine fiscale)",
        "fiches-pratiques" => "Fiches pratiques (Service-Public)",
        "autres" => "Autres publications officielles",
        _ => return None,
    })
}

/// The administrative doctrine natures: the administration's official reading
/// of a norm, invocable under its own regime, and not a norm itself.
pub const DOCTRINE_ADMIN_NATURES: &[&str] = &[
    "BOFIP",
    "CIRCULAIRE",
    "INSTRUCTION",
    "REPONSE_MINISTERIELLE",
    "RESCRIT",
];

/// The public information natures: the administration explaining the law to
/// the public, binding nobody.
pub const PUBLIC_INFO_NATURES: &[&str] = &["FICHE_PRATIQUE"];

/// The scope of a nature: `norme` | `doctrine_administrative` | `information_publique`.
pub fn nature_scope(nature: &str) -> &'static str {
    let nature = nature.to_uppercase();
    if DOCTRINE_ADMIN_NATURES.contains(&nature.as_str()) {
        "doctrine_administrative"
    } else if PUBLIC_INFO_NATURES.contains(&nature.as_str()) {
        "information_publique"
    } else {
        "norme"
    }
}

/// Nom d'un département français depuis son code canonique (`75` → « Paris »,
/// `2A`, `973`) — l'axe géographique du pot Annonces, où le code est une colonne
/// de `annonce` et non un uid de `facet_value`. `None` = code hors nomenclature
/// INSEE, servi tel quel par l'appelant.
pub fn departement_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "01" => "Ain",
        "02" => "Aisne",
        "03" => "Allier",
        "04" => "Alpes-de-Haute-Provence",
        "05" => "Hautes-Alpes",
        "06" => "Alpes-Maritimes",
        "07" => "Ardèche",
        "08" => "Ardennes",
        "09" => "Ariège",
        "10" => "Aube",
        "11" => "Aude",
        "12" => "Aveyron",
        "13" => "Bouches-du-Rhône",
        "14" => "Calvados",
        "15" => "Cantal",
        "16" => "Charente",
        "17" => "Charente-Maritime",
        "18" => "Cher",
        "19" => "Corrèze",
        "2A" => "Corse-du-Sud",
        "2B" => "Haute-Corse",
        "21" => "Côte-d'Or",
        "22" => "Côtes-d'Armor",
        "23" => "Creuse",
        "24" => "Dordogne",
        "25" => "Doubs",
        "26" => "Drôme",
        "27" => "Eure",
        "28" => "Eure-et-Loir",
        "29" => "Finistère",
        "30" => "Gard",
        "31" => "Haute-Garonne",
        "32" => "Gers",
        "33" => "Gironde",
        "34" => "Hérault",
        "35" => "Ille-et-Vilaine",
        "36" => "Indre",
        "37" => "Indre-et-Loire",
        "38" => "Isère",
        "39" => "Jura",
        "40" => "Landes",
        "41" => "Loir-et-Cher",
        "42" => "Loire",
        "43" => "Haute-Loire",
        "44" => "Loire-Atlantique",
        "45" => "Loiret",
        "46" => "Lot",
        "47" => "Lot-et-Garonne",
        "48" => "Lozère",
        "49" => "Maine-et-Loire",
        "50" => "Manche",
        "51" => "Marne",
        "52" => "Haute-Marne",
        "53" => "Mayenne",
        "54" => "Meurthe-et-Moselle",
        "55" => "Meuse",
        "56" => "Morbihan",
        "57" => "Moselle",
        "58" => "Nièvre",
        "59" => "Nord",
        "60" => "Oise",
        "61" => "Orne",
        "62" => "Pas-de-Calais",
        "63" => "Puy-de-Dôme",
        "64" => "Pyrénées-Atlantiques",
        "65" => "Hautes-Pyrénées",
        "66" => "Pyrénées-Orientales",
        "67" => "Bas-Rhin",
        "68" => "Haut-Rhin",
        "69" => "Rhône",
        "70" => "Haute-Saône",
        "71" => "Saône-et-Loire",
        "72" => "Sarthe",
        "73" => "Savoie",
        "74" => "Haute-Savoie",
        "75" => "Paris",
        "76" => "Seine-Maritime",
        "77" => "Seine-et-Marne",
        "78" => "Yvelines",
        "79" => "Deux-Sèvres",
        "80" => "Somme",
        "81" => "Tarn",
        "82" => "Tarn-et-Garonne",
        "83" => "Var",
        "84" => "Vaucluse",
        "85" => "Vendée",
        "86" => "Vienne",
        "87" => "Haute-Vienne",
        "88" => "Vosges",
        "89" => "Yonne",
        "90" => "Territoire de Belfort",
        "91" => "Essonne",
        "92" => "Hauts-de-Seine",
        "93" => "Seine-Saint-Denis",
        "94" => "Val-de-Marne",
        "95" => "Val-d'Oise",
        "971" => "Guadeloupe",
        "972" => "Martinique",
        "973" => "Guyane",
        "974" => "La Réunion",
        "975" => "Saint-Pierre-et-Miquelon",
        "976" => "Mayotte",
        "977" => "Saint-Barthélemy",
        "978" => "Saint-Martin",
        "984" => "Terres australes et antarctiques françaises",
        "986" => "Wallis-et-Futuna",
        "987" => "Polynésie française",
        "988" => "Nouvelle-Calédonie",
        "989" => "Clipperton",
        // Not departments: procurement registries serve both in the department
        // field, for Monaco and for contracts awarded outside France.
        "98" => "Monaco",
        "99" => "Étranger",
        _ => return None,
    })
}

/// Libellé FR d'une portée (facette `scope` du scope textes).
pub fn scope_label(scope: &str) -> Option<&'static str> {
    Some(match scope {
        "norme" => "Normes",
        "doctrine_administrative" => "Doctrine administrative",
        "information_publique" => "Information du public",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nature_label_is_case_insensitive() {
        assert_eq!(nature_label("LOI"), Some("Loi"));
        assert_eq!(nature_label("loi"), Some("Loi"));
        assert_eq!(nature_label("code_famille"), Some("Code de la famille"));
        assert_eq!(nature_label("CODE_FAMILLE"), Some("Code de la famille"));
        assert_eq!(nature_label("INCONNU"), None);
    }

    #[test]
    fn jurisdiction_label_covers_sentinels_and_iso() {
        assert_eq!(jurisdiction_label("FR"), Some("France"));
        assert_eq!(jurisdiction_label("EU"), Some("Union européenne"));
        assert_eq!(jurisdiction_label("INTL"), Some("International"));
        assert_eq!(jurisdiction_label("ZZ"), None);
    }

    #[test]
    fn source_label_maps_tokens_not_domains() {
        assert_eq!(source_label("legifrance"), Some("Légifrance"));
        assert_eq!(source_label("ris-at"), Some("RIS (Autriche)"));
        assert_eq!(source_label("africa-laws.org"), None);
    }
}
