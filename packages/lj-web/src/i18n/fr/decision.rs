use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::DecisionUi;

pub const DECISION: DecisionUi = DecisionUi {
    official_translation: "Traduction officielle",
    translation: "Traduction",
    automatic_translation: "Traduction automatique",
    authentic_in,
    public_rapporteur_conclusions: "Conclusions du rapporteur public",
    ai_summary: "Résumé IA",
    ai_summary_title: "Rédigé par une IA à partir de la décision.",
    analysis: "Analyse",
    analysis_title: "Publiée par la juridiction.",
    legal_summary: "Résumé juridique",
    legal_summary_title: "Rédigé par le greffe de la Cour.",
    headnote: "Sommaire",
    headnote_title: "Publié par la juridiction.",
    read_whole_summary: "Lire le résumé entier",
    fold_summary: "Replier le résumé",
    conclusions_ai_summary_title: "Rédigé par une IA à partir des conclusions.",
    read_conclusions_at,
    conclusions: "Conclusions",
    conclusions_ruling: "Voir la décision",
    print: "Impression",
    pdf: "PDF",
    reference: "Référence",
    copied: "Copié",
    copy_full_reference: "Copier la référence complète",
    copy_short_reference: "Copier la référence abrégée",
    summary: "Synthèse",
    jurisdiction: "Juridiction",
    case_name: "Nom d'usage",
    dockets: "Numéros de dossier",
    solution: "Solution",
    domain: "Domaine",
    chamber: "Chambre",
    formation: "Formation",
    hearing_date: "Date d'audience",
    decision_date: "Date de la décision",
    reading_date: "Date de lecture",
    publication: "Publication",
    nomenclature: "Nomenclature",
    publication_label,
    contents: "Sommaire",
    contents_label: "Sommaire de la décision",
    hits_in_section,
    chronology: "Chronologie de l'affaire",
    on_appeal_from: "sur appel de",
    on_cassation_appeal_against: "sur pourvoi contre",
    on_remand_after_cassation_by: "sur renvoi après cassation par",
    on_application_against: "sur requête contre",
    full_text: "Texte intégral",
    full_text_missing: "Texte intégral non disponible.",
    decision_unavailable: "Décision non disponible",
    similar: "Décisions similaires",
    similar_principle: "Décisions de principe similaires",
    similar_case: "Décisions d'espèce similaires",
    similar_abstained:
        "La décision ne tranche aucune question de droit : aucune voisine n'est proposée.",
    similar_none: "Aucun voisin suffisamment proche pour cette décision.",
    cited_by: "Citée par",
    cited_by_more,
    commentaries: "Commentaires",
    parties: "Parties",
    applicants: "Demandeurs",
    defendants: "Défendeurs",
    counsels: "Conseils",
    counsel_one: "Conseil : ",
    counsel_many: "Conseils : ",
    legal_references: "Références juridiques",
    search_selection: "Rechercher",
    copy: "Copier",
    copy_with_reference: "Copier + référence",
    not_found_title: "Décision introuvable - LibreJustice",
    courts: "Juridictions",
};

fn authentic_in(lang: &str) -> String {
    format!(", le texte qui fait foi est en {lang}")
}

fn read_conclusions_at(source: &str) -> String {
    format!("Lire les conclusions sur {source}")
}

fn publication_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "b" => "Publié au bulletin",
        "r" => "Publié au rapport",
        "l" => "Lettre de chambre",
        "c" => "Communiqué",
        "n" => "Inédit",
        "A" => "Recueil Lebon",
        "B" => "Tables du recueil Lebon",
        "C+" | "R" => "Signalée",
        "C" | "D" | "Z" => "Inédit au recueil Lebon",
        _ => return None,
    })
}

fn hits_in_section(n: usize) -> String {
    match n {
        1 => "1 occurrence des termes recherchés".to_string(),
        n => format!("{n} occurrences des termes recherchés"),
    }
}

fn cited_by_more(n: i64) -> String {
    match n {
        1 => "Une autre décision la cite.".to_string(),
        n => format!(
            "{} autres décisions la citent.",
            group_thousands(&NUMBERS, n)
        ),
    }
}
