use crate::api::Missing;
use crate::i18n::ShellUi;

pub const SHELL: ShellUi = ShellUi {
    site_description,
    skip_to_content: "Aller au contenu principal",
    site_menu: "Menu du site",
    menu: "Menu",
    home: "Accueil",
    decisions: "Décisions",
    texts: "Textes",
    directory: "Annuaire",
    notices: "Journaux",
    tools: "Outils",
    sign_in: "Connexion",
    sign_out: "Déconnexion",
    back_to_results: "Résultats",
    back_to_search: "Recherche",
    previous_result: "Résultat précédent",
    next_result: "Résultat suivant",
    theme: "Thème",
    theme_group: "Thème de l'interface",
    theme_system: "Système",
    theme_light: "Clair",
    theme_dark: "Sombre",
    language: "Langue",
    language_group: "Langue de l'interface",
    lang_offer: "Cette page existe en français.",
    lang_offer_accept: "Lire en français",
    lang_offer_dismiss: "Fermer",
    jurisdiction: "Juridiction",
    jurisdiction_of,
    state_name: |_| None,
    place_name: |_| None,
    located,
    data: "Données",
    legal_notice: "Mentions légales",
    privacy: "Confidentialité",
    terms: "Conditions d'utilisation",
    source_code: "Code source",
    support: "Soutenir",
    report_problem: "Signaler un problème",
    error: "Erreur",
    not_found: "Introuvable",
    breadcrumb: "Fil d'Ariane",
    back_home: "Retour à l'accueil",
    not_found_title: "Page introuvable - LibreJustice",
    error_404: "Erreur 404",
    page_not_found: "Page introuvable",
    not_found_body: "L'adresse demandée n'existe pas ou a été déplacée.",
    rate_limited: "Vous avez ouvert beaucoup de pages en peu de temps.",
    rate_limited_hint: "Connectez-vous pour continuer à lire, ou réessayez dans une minute.",
    reload: "Recharger",
    later: "Plus tard",
    new_version: "Une nouvelle version est disponible.",
    page_incomplete: "Cette page n'a pas pu charger entièrement.",
    invalid_id: "Identifiant invalide",
    invalid_entity: "Entité invalide",
    missing,
};

fn missing(missing: Missing) -> &'static str {
    match missing {
        Missing::Decision => "Décision introuvable",
        Missing::Article => "Article introuvable",
        Missing::Places => "Lieux introuvables",
        Missing::Corpus => "Corpus introuvable",
        Missing::Version => "Version introuvable",
        Missing::Code => "Code introuvable",
        Missing::Consultation => "Consultation introuvable",
        Missing::Notice => "Annonce introuvable",
        Missing::Section => "Section introuvable",
        Missing::Entity => "Entité introuvable",
    }
}

fn jurisdiction_of(name: &str) -> String {
    format!("Juridiction : {name}")
}

fn site_description(code: &str) -> String {
    match (code, super::law_adjective(code)) {
        ("fr", _) => "Moteur de recherche libre sur le droit français : jurisprudence (Conseil d'État, Cour de cassation, cours d'appel, tribunaux) et textes (codes, lois, traités), mis à jour quotidiennement.".to_string(),
        (_, Some(adjective)) => format!("Moteur de recherche libre sur le droit {adjective} : jurisprudence et textes, mis à jour quotidiennement."),
        (_, None) => "Moteur de recherche libre sur le droit : jurisprudence et textes, mis à jour quotidiennement.".to_string(),
    }
}

fn located(code: &str, name: &str) -> String {
    let preposition = match code {
        "BF" | "BI" | "BJ" | "CA" | "CG" | "CM" | "DK" | "EH" | "GA" | "GB" | "GH" | "KE"
        | "LR" | "LS" | "LU" | "MA" | "ML" | "MW" | "MZ" | "NE" | "NG" | "PE" | "PT" | "RW"
        | "SN" | "TD" | "TG" | "VE" | "VN" => "au ",
        "KM" | "NL" => "aux ",
        "BL" | "CY" | "DJ" | "MC" | "MF" | "MG" | "MT" | "MU" | "PM" | "RE" | "ST" | "WF"
        | "YT" => "à ",
        "CH-AG" | "CH-AI" | "CH-AR" | "CH-OW" | "CH-UR" => "dans le canton d'",
        "CH-JU" | "CH-TI" | "CH-VS" => "dans le canton du ",
        "CH-GR" => "dans le canton des ",
        canton if canton.starts_with("CH-") => "dans le canton de ",
        "OHADA" | "UEMOA" | "UNIONAFRICAINE" => "dans l'",
        "CEMAC" | "CEDEAO" => "dans la ",
        "TF" => "dans les ",
        _ => "en ",
    };
    format!("{preposition}{name}")
}
