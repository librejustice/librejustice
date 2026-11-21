use super::numbers::NUMBERS;
use crate::helpers::group_thousands;
use crate::i18n::DecisionUi;

pub const DECISION: DecisionUi = DecisionUi {
    official_translation: "Amtliche Übersetzung",
    translation: "Übersetzung",
    automatic_translation: "Automatische Übersetzung",
    authentic_in,
    public_rapporteur_conclusions: "Schlussanträge des Rapporteur public",
    ai_summary: "KI-Zusammenfassung",
    ai_summary_title: "Von einer KI aus dem Entscheid erstellt.",
    analysis: "Analyse",
    analysis_title: "Vom Gericht publiziert.",
    legal_summary: "Rechtliche Zusammenfassung",
    legal_summary_title: "Von der Kanzlei des Gerichtshofs verfasst.",
    headnote: "Regeste",
    headnote_title: "Vom Gericht publiziert.",
    read_whole_summary: "Ganze Zusammenfassung lesen",
    fold_summary: "Zusammenfassung ausblenden",
    conclusions_ai_summary_title: "Von einer KI aus den Schlussanträgen erstellt.",
    read_conclusions_at,
    conclusions: "Schlussanträge",
    conclusions_ruling: "Zur Entscheidung",
    print: "Drucken",
    pdf: "PDF",
    reference: "Referenz",
    copied: "Kopiert",
    copy_full_reference: "Vollständige Referenz kopieren",
    copy_short_reference: "Kurzreferenz kopieren",
    summary: "Übersicht",
    jurisdiction: "Gericht",
    case_name: "Gebräuchliche Bezeichnung",
    dockets: "Geschäftsnummern",
    solution: "Verfahrensausgang",
    domain: "Rechtsgebiet",
    chamber: "Abteilung",
    formation: "Besetzung",
    hearing_date: "Verhandlungsdatum",
    decision_date: "Entscheiddatum",
    reading_date: "Datum der Verkündung",
    publication: "Publikation",
    nomenclature: "Nomenklatur",
    publication_label,
    contents: "Inhaltsverzeichnis",
    contents_label: "Inhaltsverzeichnis des Entscheids",
    hits_in_section,
    chronology: "Instanzenzug",
    on_appeal_from: "auf Berufung gegen",
    on_cassation_appeal_against: "auf Kassationsbeschwerde gegen",
    on_remand_after_cassation_by: "nach Kassation und Rückweisung durch",
    on_application_against: "auf Beschwerde gegen",
    full_text: "Volltext",
    full_text_missing: "Volltext nicht verfügbar.",
    decision_unavailable: "Entscheid nicht verfügbar",
    similar: "Ähnliche Entscheide",
    similar_principle: "Ähnliche Leitentscheide",
    similar_case: "Ähnliche Einzelfallentscheide",
    similar_abstained:
        "Der Entscheid beantwortet keine Rechtsfrage: Es werden keine ähnlichen Entscheide vorgeschlagen.",
    similar_none: "Für diesen Entscheid wurde kein ausreichend ähnlicher gefunden.",
    cited_by: "Zitiert in",
    cited_by_more,
    commentaries: "Kommentare",
    parties: "Parteien",
    applicants: "Klagende Parteien",
    defendants: "Beklagte Parteien",
    counsels: "Rechtsvertretung",
    counsel_one: "Rechtsvertretung: ",
    counsel_many: "Rechtsvertretung: ",
    legal_references: "Zitierte Erlasse",
    search_selection: "Suchen",
    copy: "Kopieren",
    copy_with_reference: "Kopieren + Referenz",
    not_found_title: "Entscheid nicht gefunden - LibreJustice",
    courts: "Gerichte",
};

fn authentic_in(lang: &str) -> String {
    format!(", massgebend ist der Text auf {lang}")
}

fn read_conclusions_at(source: &str) -> String {
    format!("Schlussanträge auf {source} lesen")
}

fn publication_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "b" => "Im Bulletin publiziert",
        "r" => "Im Jahresbericht publiziert",
        "l" => "Lettre de chambre",
        "c" => "Medienmitteilung",
        "n" => "Nicht publiziert",
        "A" => "Recueil Lebon",
        "B" => "Tables du recueil Lebon",
        "C+" | "R" => "Hervorgehoben",
        "C" | "D" | "Z" => "Nicht im Recueil Lebon publiziert",
        _ => return None,
    })
}

fn hits_in_section(n: usize) -> String {
    match n {
        1 => "1 Treffer für die Suchbegriffe".to_string(),
        n => format!("{n} Treffer für die Suchbegriffe"),
    }
}

fn cited_by_more(n: i64) -> String {
    match n {
        1 => "Ein weiterer Entscheid zitiert ihn.".to_string(),
        n => format!(
            "{} weitere Entscheide zitieren ihn.",
            group_thousands(&NUMBERS, n)
        ),
    }
}
