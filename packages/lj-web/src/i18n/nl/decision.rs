use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::DecisionUi;

pub const DECISION: DecisionUi = DecisionUi {
    official_translation: "Officiële vertaling",
    translation: "Vertaling",
    automatic_translation: "Automatische vertaling",
    authentic_in,
    public_rapporteur_conclusions: "Conclusie van de rapporteur public",
    ai_summary: "AI-samenvatting",
    ai_summary_title: "Door een AI geschreven op basis van de beslissing.",
    analysis: "Analyse",
    analysis_title: "Gepubliceerd door het rechtscollege.",
    legal_summary: "Juridische samenvatting",
    legal_summary_title: "Opgesteld door de griffie van het Hof.",
    headnote: "Samenvatting",
    headnote_title: "Gepubliceerd door het rechtscollege.",
    read_whole_summary: "De volledige samenvatting lezen",
    fold_summary: "Samenvatting inklappen",
    conclusions_ai_summary_title: "Door een AI geschreven op basis van de conclusie.",
    read_conclusions_at,
    conclusions: "Conclusies",
    conclusions_ruling: "Naar de uitspraak",
    print: "Afdrukken",
    pdf: "PDF",
    reference: "Referentie",
    copied: "Gekopieerd",
    copy_full_reference: "Volledige referentie kopiëren",
    copy_short_reference: "Verkorte referentie kopiëren",
    summary: "Overzicht",
    jurisdiction: "Rechtscollege",
    case_name: "Gangbare naam",
    dockets: "Zaaknummers",
    solution: "Uitkomst",
    domain: "Rechtsgebied",
    chamber: "Kamer",
    formation: "Samenstelling",
    hearing_date: "Datum van de zitting",
    decision_date: "Datum van de beslissing",
    reading_date: "Datum van de uitspraak",
    publication: "Publicatie",
    nomenclature: "Nomenclatuur",
    publication_label,
    contents: "Inhoudsopgave",
    contents_label: "Inhoudsopgave van de beslissing",
    hits_in_section,
    chronology: "Procesverloop",
    on_appeal_from: "in hoger beroep van",
    on_cassation_appeal_against: "op cassatieberoep tegen",
    on_remand_after_cassation_by: "na cassatie en verwijzing door",
    on_application_against: "op verzoekschrift tegen",
    full_text: "Volledige tekst",
    full_text_missing: "Volledige tekst niet beschikbaar.",
    decision_unavailable: "Beslissing niet beschikbaar",
    similar: "Vergelijkbare beslissingen",
    similar_principle: "Vergelijkbare principiële beslissingen",
    similar_case: "Vergelijkbare beslissingen in concrete zaken",
    similar_abstained:
        "De beslissing beslecht geen rechtsvraag: er worden geen vergelijkbare beslissingen voorgesteld.",
    similar_none: "Geen beslissing die er voldoende op lijkt.",
    cited_by: "Aangehaald in",
    cited_by_more,
    commentaries: "Commentaren",
    parties: "Partijen",
    applicants: "Eisers",
    defendants: "Verweerders",
    counsels: "Raadslieden",
    counsel_one: "Raadsman: ",
    counsel_many: "Raadslieden: ",
    legal_references: "Juridische verwijzingen",
    search_selection: "Zoeken",
    copy: "Kopiëren",
    copy_with_reference: "Kopiëren + referentie",
    not_found_title: "Beslissing niet gevonden - LibreJustice",
    courts: "Rechtscolleges",
};

fn authentic_in(lang: &str) -> String {
    format!(", de authentieke tekst is in het {lang}")
}

fn read_conclusions_at(source: &str) -> String {
    format!("De conclusie lezen op {source}")
}

fn publication_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "b" => "Gepubliceerd in het Bulletin",
        "r" => "Gepubliceerd in het Rapport",
        "l" => "Lettre de chambre",
        "c" => "Persbericht",
        "n" => "Niet gepubliceerd",
        "A" => "Recueil Lebon",
        "B" => "Tables du recueil Lebon",
        "C+" | "R" => "Gesignaleerd",
        "C" | "D" | "Z" => "Niet gepubliceerd in het Recueil Lebon",
        _ => return None,
    })
}

fn hits_in_section(n: usize) -> String {
    match n {
        1 => "1 treffer voor de zoektermen".to_string(),
        n => format!("{n} treffers voor de zoektermen"),
    }
}

fn cited_by_more(n: i64) -> String {
    match n {
        1 => "Nog één andere beslissing haalt ze aan.".to_string(),
        n => format!(
            "Nog {} andere beslissingen halen ze aan.",
            group_thousands(&NUMBERS, n)
        ),
    }
}
