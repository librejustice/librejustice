//! Libellés FR d'affichage du référentiel de textes (`legal_text` /
//! `legal_article`) — mappings purs valeur DB → libellé humain, pour les
//! facettes de `/textes` et le catalogue des codes.
//!
//! Trois axes :
//! - `jurisdiction` : ISO 3166-1 alpha-2 + sentinelles `UE`/`INTL` → nom FR ;
//! - `nature` : jetons LEGI (`LOI_ORGANIQUE`) et catégories curées
//!   (`code_famille`) → libellé FR — l'entrée est normalisée en majuscules
//!   (le corpus curé mélange `loi`/`LOI`, `constitution`/`CONSTITUTION`) ;
//! - `source` : les diffuseurs à jeton court (`legifrance`, `kali`, `ris-at`…)
//!   reçoivent un libellé ; un domaine (`sgg-mali.ml`, `africa-laws.org`…) se
//!   lit tel quel.
//!
//! `None` = valeur inconnue du mapping, l'appelant retombe sur la valeur brute.

/// Nom FR d'une juridiction du référentiel (code ISO alpha-2, `UE`, `INTL`).
pub fn jurisdiction_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "FR" => "France",
        "UE" => "Union européenne",
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
        "ARRETE" => "Arrêté",
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
        "CODE_NATIONALITE" => "Code de la nationalité",
        "CODE_OBLIGATIONS" => "Code des obligations",
        "CODE_PENAL" => "Code pénal",
        "CODE_PERSONNES" => "Code des personnes",
        "CODE_PROCEDURE" => "Code de procédure",
        "CODE_PROCEDURE_CIVILE" => "Code de procédure civile",
        "CODE_PROCEDURE_PENALE" => "Code de procédure pénale",
        "CODE_TRAVAIL" => "Code du travail",
        "CONSTITUTION" => "Constitution",
        "CONVENTION" => "Convention",
        "DECISION" => "Décision",
        "DECRET" => "Décret",
        "DECRET_LOI" => "Décret-loi",
        "DELIBERATION" => "Délibération",
        "DIRECTIVE" => "Directive",
        "DIRECTIVE_EURO" => "Directive européenne",
        "ETAT_CIVIL" => "État civil",
        "IDCC" => "Convention collective",
        "INSTRUCTION" => "Instruction",
        "LOI" => "Loi",
        "LOI_CONSTIT" => "Loi constitutionnelle",
        "LOI_ORGANIQUE" => "Loi organique",
        "LOI_PROGRAMME" => "Loi de programmation",
        "ORDONNANCE" => "Ordonnance",
        "PROTOCOLE" => "Protocole",
        "RAPPORT" => "Rapport",
        "REGLEMENT" => "Règlement",
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

/// Fonds du catalogue des normes (ADR 0255, `/normes`), dans l'ordre
/// d'affichage. `codes` renvoie vers le catalogue `/codes` existant ; les
/// autres fonds ont des hubs année (`/normes/{fond}/{annee}`). L'affectation
/// nature → fond vit dans le fragment SQL de `lj-store::norm_hubs` (seule
/// source, utilisée par l'index d'expression de la migration 0166).
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
        "autres" => "Autres publications officielles",
        _ => return None,
    })
}

/// Natures « doctrine administrative » (ADR 0196) : interprétations officielles
/// de la norme par l'administration (opposables/invocables dans leurs régimes),
/// par opposition aux normes elles-mêmes. Pilote la sur-facette `scope` du
/// scope textes ; toute nature hors liste est une norme.
pub const DOCTRINE_ADMIN_NATURES: &[&str] = &[
    "BOFIP",
    "CIRCULAIRE",
    "INSTRUCTION",
    "REPONSE_MINISTERIELLE",
    "RESCRIT",
];

/// Portée d'une nature (ADR 0196) : `norme` | `doctrine_administrative`.
pub fn nature_scope(nature: &str) -> &'static str {
    if DOCTRINE_ADMIN_NATURES.contains(&nature.to_uppercase().as_str()) {
        "doctrine_administrative"
    } else {
        "norme"
    }
}

/// Libellé FR d'une portée (facette `scope` du scope textes).
pub fn scope_label(scope: &str) -> Option<&'static str> {
    Some(match scope {
        "norme" => "Normes",
        "doctrine_administrative" => "de référence administrative",
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
        assert_eq!(jurisdiction_label("UE"), Some("Union européenne"));
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
